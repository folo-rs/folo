//! Real Git snapshots and loopback forge calls for tag failure and operator recovery.

use std::fs;
use std::sync::{Arc, Mutex};

use crp_impl::publication::context::WorkflowRun;
use crp_impl::publication::github::{Github, GithubOutcome, GithubState, reconcile_with};
use crp_impl::publication::manifest::PublicationManifest;
use crp_impl::publication::registry::RegistryClient;
use crp_impl::verbose::Verbose;
use serde_json::{Value, json};
use tiny_http::{Method, Response, StatusCode};

use crate::git_fixture::Repository;
use crate::http_fixture::HttpService;

#[test]
#[cfg_attr(miri, ignore = "Uses Git, Cargo and loopback registry/forge services")]
fn failed_tag_does_not_suppress_other_releases_and_manual_tag_allows_retry() {
    let repository = Repository::new();
    repository.write(
        "Cargo.toml",
        b"[workspace]\nmembers=['alpha','beta']\nresolver='3'\n",
    );
    for name in ["alpha", "beta"] {
        repository.write(
            &format!("{name}/Cargo.toml"),
            format!("[package]\nname='{name}'\nversion='1.0.0'\nedition='2024'\n").as_bytes(),
        );
        repository.write(&format!("{name}/src/lib.rs"), b"pub fn ready() {}\n");
    }
    repository.write(
        ".cargo/release_plan.toml",
        b"schema-version=1\nrepository='example/releases'\nrelease-branch='stable'\ntargets=[]\n",
    );
    repository.command(&["add", "."]);
    repository.command(&["commit", "--quiet", "-m", "publication source"]);
    let source = repository.command(&["rev-parse", "HEAD"]).trim().to_owned();
    repository.write(
        "alpha/Cargo.toml",
        b"[package]\nname='alpha'\nversion='1.0.1'\nedition='2024'\n",
    );
    repository.command(&["add", "."]);
    repository.command(&["commit", "--quiet", "-m", "next alpha version"]);
    let candidate = repository.command(&["rev-parse", "HEAD"]).trim().to_owned();
    repository.command(&["branch", "stable", "HEAD"]);
    repository.command(&["checkout", "--detach", &source]);
    repository.command(&[
        "config",
        &format!("url.{}.insteadOf", repository.path().display()),
        "https://github.com/example/releases.git",
    ]);
    let publication: PublicationManifest = PublicationManifest::new(serde_json::from_value(json!({
        "schema_version":1,"tool_version":"1.0.0","source":source,
        "workspace_manifest":"Cargo.toml","config_path":".cargo/release_plan.toml",
        "configuration":{"schema-version":1,"repository":"example/releases","release-branch":"stable","targets":[]},
        "packages":[
            {"name":"alpha","version":"1.0.0","manifest":"alpha/Cargo.toml","binary":null},
            {"name":"beta","version":"1.0.0","manifest":"beta/Cargo.toml","binary":null}
        ]
    })).unwrap()).unwrap();
    let tags = Arc::new(Mutex::new(
        std::collections::BTreeMap::<String, String>::new(),
    ));
    let registry_ready = Arc::new(Mutex::new(false));
    let service = HttpService::new({
        let tags = Arc::clone(&tags);
        let registry_ready = Arc::clone(&registry_ready);
        move |_, mut request| {
            let path = request.url().to_owned();
            let (status, body) = if path.starts_with("/index/") {
                let name = path.rsplit('/').next().unwrap();
                if *registry_ready.lock().unwrap() {
                    (200, json!({"name":name,"vers":"1.0.0"}))
                } else {
                    (404, json!({}))
                }
            } else if path == "/repos/example/releases" {
                (200, json!({"full_name":"example/releases"}))
            } else if let Some(tag) = path.strip_prefix("/repos/example/releases/git/ref/tags/") {
                match tags.lock().unwrap().get(tag) {
                    Some(source) => (200, json!({"object":{"type":"commit","sha":source}})),
                    None => (404, json!({})),
                }
            } else if path == "/repos/example/releases/git/refs" {
                assert_eq!(request.method(), &Method::Post);
                let mut body = String::new();
                request.as_reader().read_to_string(&mut body).unwrap();
                let body: Value = serde_json::from_str(&body).unwrap();
                assert_eq!(body.get("ref").unwrap(), "refs/tags/beta-v1.0.0");
                assert_eq!(body.get("sha").unwrap(), &candidate);
                tags.lock()
                    .unwrap()
                    .insert("beta-v1.0.0".to_owned(), candidate.clone());
                (201, json!({"ref":"refs/tags/beta-v1.0.0"}))
            } else {
                panic!("Unexpected request: {path}")
            };
            request
                .respond(
                    Response::from_string(body.to_string()).with_status_code(StatusCode(status)),
                )
                .unwrap();
        }
    });
    let registry = RegistryClient::with_endpoint(&format!("{}/index", service.url())).unwrap();
    let github = Github::with_endpoint(
        service.url(),
        "example/releases",
        Some("fixture-token".to_owned()),
    )
    .unwrap();
    let output = tempfile::tempdir().unwrap();
    let mut unavailable = outcome(&publication);
    reconcile_with(
        &publication,
        &repository.path().join("Cargo.toml"),
        &output.path().join("unavailable"),
        &mut unavailable,
        Verbose::new(false),
        &registry,
        &github,
    )
    .unwrap_err();
    assert!(tags.lock().unwrap().is_empty());
    assert!(unavailable.packages.is_empty());
    *registry_ready.lock().unwrap() = true;
    let mut first = outcome(&publication);
    reconcile_with(
        &publication,
        &repository.path().join("Cargo.toml"),
        &output.path().join("first"),
        &mut first,
        Verbose::new(false),
        &registry,
        &github,
    )
    .unwrap();
    assert_eq!(first.packages.first().unwrap().state, GithubState::Failed);
    assert_eq!(
        first.packages.first().unwrap().observed_version.as_deref(),
        Some("1.0.1")
    );
    assert_eq!(
        first.packages.first().unwrap().recovery_source.as_deref(),
        Some(source.as_str())
    );
    assert_eq!(first.packages.last().unwrap().state, GithubState::Complete);
    assert!(tags.lock().unwrap().contains_key("beta-v1.0.0"));
    assert!(!first.errors.is_empty());
    tags.lock()
        .unwrap()
        .insert("alpha-v1.0.0".to_owned(), source.clone());
    let mut retry = outcome(&publication);
    reconcile_with(
        &publication,
        &repository.path().join("Cargo.toml"),
        &output.path().join("retry"),
        &mut retry,
        Verbose::new(false),
        &registry,
        &github,
    )
    .unwrap();
    assert!(retry.errors.is_empty());
    assert!(
        retry
            .packages
            .iter()
            .all(|package| package.state == GithubState::Complete)
    );
    assert_eq!(
        retry.packages.first().unwrap().source.as_deref(),
        Some(source.as_str())
    );
    assert_eq!(
        repository
            .command(&["worktree", "list", "--porcelain"])
            .matches("worktree ")
            .count(),
        1
    );
}

#[test]
#[cfg_attr(miri, ignore = "Uses Git, Cargo and loopback registry/forge services")]
fn existing_binary_tag_emits_only_incomplete_native_pairs_and_dry_run_never_creates_release() {
    let repository = Repository::new();
    repository.write(
        "Cargo.toml",
        br#"[package]
name="tool"
version="1.0.0"
edition="2024"
repository="https://github.com/example/releases"
[[bin]]
name="tool-command"
path="src/main.rs"
[package.metadata.binstall]
pkg-url="{ repo }/releases/download/{ name }-v{ version }/{ name }-v{ version }-{ target }.zip"
bin-dir="{ bin }{ binary-ext }"
pkg-fmt="zip"
"#,
    );
    repository.write("src/main.rs", b"fn main() {}\n");
    repository.write(
        ".cargo/release_plan.toml",
        b"schema-version=1\nrepository='example/releases'\nrelease-branch='stable'\n\
          targets=['x86_64-pc-windows-msvc','x86_64-unknown-linux-gnu']\n",
    );
    repository.command(&["add", "."]);
    repository.command(&["commit", "--quiet", "-m", "binary source"]);
    let source = repository.command(&["rev-parse", "HEAD"]).trim().to_owned();
    let publication = PublicationManifest::new(serde_json::from_value(json!({
        "schema_version":1,"tool_version":"1.0.0","source":source,
        "workspace_manifest":"Cargo.toml","config_path":".cargo/release_plan.toml",
        "configuration":{"schema-version":1,"repository":"example/releases","release-branch":"stable",
            "targets":["x86_64-pc-windows-msvc","x86_64-unknown-linux-gnu"]},
        "packages":[{"name":"tool","version":"1.0.0","manifest":"Cargo.toml",
            "binary":{"name":"tool-command","targets":["x86_64-pc-windows-msvc","x86_64-unknown-linux-gnu"]}}]
    })).unwrap()).unwrap();
    let created = Arc::new(Mutex::new(false));
    let service = HttpService::new({
        let created = Arc::clone(&created);
        let source = source.clone();
        move |_, mut request| {
            let (status, body) = match request.url() {
                "/index/to/ol/tool" => (200, json!({"name":"tool","vers":"1.0.0"})),
                "/repos/example/releases" => (200, json!({"full_name":"example/releases"})),
                "/repos/example/releases/git/ref/tags/tool-v1.0.0" => {
                    (200, json!({"object":{"type":"commit","sha":source}}))
                }
                "/repos/example/releases/releases/tags/tool-v1.0.0"
                    if !*created.lock().unwrap() =>
                {
                    (404, json!({}))
                }
                "/repos/example/releases/releases/tags/tool-v1.0.0" => {
                    (200, json!({"id":7,"tag_name":"tool-v1.0.0","draft":false}))
                }
                "/repos/example/releases/releases" => {
                    assert_eq!(request.method(), &Method::Post);
                    let mut body = String::new();
                    request.as_reader().read_to_string(&mut body).unwrap();
                    let body: Value = serde_json::from_str(&body).unwrap();
                    assert_eq!(body.get("tag_name").unwrap(), "tool-v1.0.0");
                    assert_eq!(body.get("target_commitish").unwrap(), &source);
                    *created.lock().unwrap() = true;
                    (201, json!({"id":7,"tag_name":"tool-v1.0.0","draft":false}))
                }
                "/repos/example/releases/releases/7/assets?per_page=100&page=1" => (
                    200,
                    Value::Array(
                        (0..100)
                            .map(|index| {
                                json!({
                                    "name":format!("operator-asset-{index}"),"state":"uploaded"
                                })
                            })
                            .collect(),
                    ),
                ),
                "/repos/example/releases/releases/7/assets?per_page=100&page=2" => (
                    200,
                    json!([
                        {"name":"tool-v1.0.0-x86_64-pc-windows-msvc.zip","state":"uploaded"},
                        {"name":"tool-v1.0.0-x86_64-pc-windows-msvc.sha256","state":"uploaded"},
                        {"name":"tool-v1.0.0-x86_64-unknown-linux-gnu.zip","state":"uploaded"}
                    ]),
                ),
                path => panic!("Unexpected forge request: {path}"),
            };
            request
                .respond(
                    Response::from_string(body.to_string()).with_status_code(StatusCode(status)),
                )
                .unwrap();
        }
    });
    let registry = RegistryClient::with_endpoint(&format!("{}/index", service.url())).unwrap();
    let github = Github::with_endpoint(
        service.url(),
        "example/releases",
        Some("fixture-token".to_owned()),
    )
    .unwrap();
    let output = tempfile::tempdir().unwrap();
    let mut dry = outcome(&publication);
    dry.dry_run = true;
    reconcile_with(
        &publication,
        &repository.path().join("Cargo.toml"),
        &output.path().join("dry"),
        &mut dry,
        Verbose::new(false),
        &registry,
        &github,
    )
    .unwrap();
    assert_eq!(
        dry.packages.first().unwrap().state,
        GithubState::WouldCreateRelease
    );
    assert!(!*created.lock().unwrap());
    assert!(!output.path().join("dry").exists());
    let mut result = outcome(&publication);
    reconcile_with(
        &publication,
        &repository.path().join("Cargo.toml"),
        &output.path().join("batches"),
        &mut result,
        Verbose::new(false),
        &registry,
        &github,
    )
    .unwrap();
    assert!(result.errors.is_empty());
    assert_eq!(result.batches.len(), 1);
    let batch = result.batches.first().unwrap();
    assert_eq!(batch.target, "x86_64-unknown-linux-gnu");
    let body: Value =
        serde_json::from_slice(&fs::read(output.path().join("batches").join(&batch.path)).unwrap())
            .unwrap();
    assert_eq!(body.get("publication_id").unwrap(), &publication.id);
    assert_eq!(body.pointer("/binaries/0/source_sha").unwrap(), &source);
    assert_eq!(body.pointer("/binaries/0/bin").unwrap(), "tool-command");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Uses the GitHub client against a loopback issue service"
)]
fn failure_issue_is_run_qualified_and_reused_after_retry() {
    let issue = Arc::new(Mutex::new(None::<Value>));
    let methods = Arc::new(Mutex::new(Vec::new()));
    let service = HttpService::new({
        let issue = Arc::clone(&issue);
        let methods = Arc::clone(&methods);
        move |_, mut request| {
            let path = request.url().to_owned();
            let body = match (request.method(), path.as_str()) {
                (&Method::Get, "/repos/example/releases") => {
                    json!({"full_name":"example/releases"})
                }
                (&Method::Get, "/repos/example/releases/issues?state=all&per_page=100&page=1") => {
                    json!(issue.lock().unwrap().iter().cloned().collect::<Vec<_>>())
                }
                (&Method::Post, "/repos/example/releases/issues")
                | (&Method::Patch, "/repos/example/releases/issues/7") => {
                    methods.lock().unwrap().push(request.method().clone());
                    let mut body = String::new();
                    request.as_reader().read_to_string(&mut body).unwrap();
                    let mut body: Value = serde_json::from_str(&body).unwrap();
                    if body.get("title").is_none() {
                        let title = issue
                            .lock()
                            .unwrap()
                            .as_ref()
                            .unwrap()
                            .get("title")
                            .unwrap()
                            .clone();
                        body.as_object_mut()
                            .unwrap()
                            .insert("title".to_owned(), title);
                    }
                    body.as_object_mut()
                        .unwrap()
                        .insert("number".to_owned(), json!(7));
                    *issue.lock().unwrap() = Some(body.clone());
                    body
                }
                (_, path) => panic!("Unexpected issue request: {path}"),
            };
            request
                .respond(Response::from_string(body.to_string()))
                .unwrap();
        }
    });
    let github = Github::with_endpoint(
        service.url(),
        "example/releases",
        Some("fixture".to_owned()),
    )
    .unwrap();
    for attempt in [1, 2] {
        let context: WorkflowRun =
            serde_json::from_value(json!({"run_id":123,"run_attempt":attempt})).unwrap();
        github.report_failure(context,"[Copilot speaking]\nCreate tool-v1.0.0 at the recorded immutable source, then retry.").unwrap();
    }
    assert_eq!(*methods.lock().unwrap(), [Method::Post, Method::Patch]);
    let issue = issue.lock().unwrap();
    let issue = issue.as_ref().unwrap();
    assert_eq!(
        issue.get("title").unwrap(),
        "Release failed: workflow run 123"
    );
    assert!(
        issue
            .get("body")
            .unwrap()
            .as_str()
            .unwrap()
            .contains("cargo-release-plan:123:2")
    );
}

fn outcome(publication: &PublicationManifest) -> GithubOutcome {
    GithubOutcome {
        schema_version: 1,
        publication_id: publication.id.clone(),
        phase: "github".to_owned(),
        dry_run: false,
        complete: false,
        packages: Vec::new(),
        batches: Vec::new(),
        planned_targets: Vec::new(),
        errors: Vec::new(),
        github: None,
    }
}
