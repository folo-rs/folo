//! Cargo's credential protocol and private lease cleanup against an isolated identity service.

use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crp_publication::publication::credentials::{CredentialSession, serve_credential};
use crp_publication::publication::identity::{ActionsIdentity, TrustedPublisher};
use crp_publication::publication::manifest::{Publication, PublicationManifest};
use serde_json::{Value, json};

use crate::git_fixture::Repository;
use crate::identity_fixture::{IdentityService, LeaseService};

#[test]
#[cfg_attr(
    miri,
    ignore = "Executes a child with the publication credential configuration"
)]
fn configured_child_receives_no_orchestration_credentials() {
    testing::with_watchdog(|| {
        let directory = tempfile::tempdir().unwrap();
        let publication = PublicationManifest::new(serde_json::from_value(json!({
            "schema_version":1,"tool_version":"1.0.0","source":"a".repeat(40),
            "workspace_manifest":"Cargo.toml","config_path":".cargo/release_plan.toml",
            "configuration":{"schema-version":1,"repository":"example/library","release-branch":"main","targets":[]},
            "packages":[]
        })).unwrap()).unwrap();
        let session = CredentialSession::new(
            serde_json::from_value(json!({
                "request_url":"https://example.invalid/identity",
                "request_token":"unused-identity-canary"
            }))
            .unwrap(),
            publication,
            directory.path().join("Cargo.toml"),
            TrustedPublisher::with_endpoint(
                "https://example.invalid/tokens",
                crp_publication::PublicationOutput::new(
                    "1.0.0",
                    false,
                    std::sync::Arc::new(crp_diag::Discard),
                ),
            )
            .unwrap(),
        )
        .unwrap();
        // Inspect the actual child environment without invoking publication or an identity service.
        // The trailing arguments supplied by configure are inert script arguments in this probe.
        let mut command = Command::new("pwsh");
        command.args([
            "-NoProfile",
            "-CommandWithArgs",
            r#"Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true
foreach ($name in @('GH_TOKEN', 'GITHUB_TOKEN', 'GIT_TOKEN', 'INPUT_TOKEN', 'DEFAULT_GITHUB_TOKEN',
    'CARGO_REGISTRY_TOKEN', 'ACTIONS_ID_TOKEN_REQUEST_URL', 'ACTIONS_ID_TOKEN_REQUEST_TOKEN')) {
    if ([Environment]::GetEnvironmentVariable($name) -ne $null) { throw "Credential survived: $name" }
}
if ($env:CARGO_REGISTRIES_PRIVATE_INDEX -ne 'index-canary') { throw 'Registry configuration removed' }
if (-not (Test-Path -LiteralPath $env:CARGO_RELEASE_PLAN_CREDENTIAL_CONTEXT)) { throw 'Provider context missing' }
"#,
        ]);
        for name in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GIT_TOKEN",
            "INPUT_TOKEN",
            "DEFAULT_GITHUB_TOKEN",
            "CARGO_REGISTRY_TOKEN",
            "ACTIONS_ID_TOKEN_REQUEST_URL",
            "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
        ] {
            command.env(name, "must-not-reach-child");
        }
        command.env("CARGO_REGISTRIES_PRIVATE_INDEX", "index-canary");
        session
            .configure(&mut command, Path::new("provider"))
            .unwrap();
        let output = command.output().unwrap();
        let cleanup = session.finish();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        cleanup.unwrap();
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Invokes Cargo with an isolated credential configuration"
)]
fn cargo_uses_the_crates_io_provider_override_not_the_named_registry_key() {
    let directory = tempfile::tempdir().unwrap();
    let missing = directory.path().join("missing-credential-provider");
    let provider = serde_json::to_string(&[missing.to_str().unwrap()]).unwrap();
    let output = Command::new("cargo")
        .args(["login", "--registry", "crates-io", "--config"])
        .arg(format!("registry.credential-provider={provider}"))
        .env("CARGO_HOME", directory.path())
        .current_dir(directory.path())
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("missing-credential-provider"));
}

#[test]
#[cfg_attr(miri, ignore = "Uses owned credential files, Git and loopback HTTP")]
fn provider_issues_only_requested_credentials_and_parent_revokes_the_lease() {
    for (binary, reject_revocation) in [(false, false), (true, false), (true, true)] {
        let service = IdentityService::with_failures(false, reject_revocation);
        let repository = Repository::new();
        repository.write(
            "Cargo.toml",
            b"[package]\nname='library'\nversion='1.0.0'\nedition='2024'\n",
        );
        repository.write("src/lib.rs", b"pub fn ready() {}\n");
        if binary {
            repository.write("src/main.rs", b"fn main() {}\n");
        }
        repository.command(&["add", "."]);
        repository.command(&["commit", "--quiet", "-m", "credential source"]);
        let source = repository.command(&["rev-parse", "HEAD"]).trim().to_owned();
        let publication: Publication = serde_json::from_value(json!({
        "schema_version":1, "tool_version":"1.0.0", "source":source,
        "workspace_manifest":"Cargo.toml", "config_path":".cargo/release_plan.toml",
        "configuration":{"schema-version":1,"repository":"example/library","release-branch":"main","targets":["x86_64-unknown-linux-gnu"]},
        "packages":[{"name":"library","version":"1.0.0","manifest":"Cargo.toml",
            "binary":if binary { json!({"name":"library","targets":["x86_64-unknown-linux-gnu"]}) } else { Value::Null }}]
    }))
    .unwrap();
        let identity: ActionsIdentity = serde_json::from_value(json!({
            "request_url":format!("{}/identity",service.url()),
            "request_token":"identity-credential-canary"
        }))
        .unwrap();
        let publisher = TrustedPublisher::with_endpoint(
            &format!("{}/tokens", service.url()),
            crp_publication::PublicationOutput::new(
                "1.2.3",
                false,
                std::sync::Arc::new(crp_diag::Discard),
            ),
        )
        .unwrap();
        let session = CredentialSession::new(
            identity,
            PublicationManifest::new(publication).unwrap(),
            repository.path().join("Cargo.toml"),
            publisher,
        )
        .unwrap();
        let mut command = Command::new("cargo");
        session
            .configure(&mut command, Path::new("provider"))
            .unwrap();
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["--config", "registry.credential-provider=[\"provider\"]"]
        );
        let context = command
            .get_envs()
            .find(|(name, _)| *name == "CARGO_RELEASE_PLAN_CREDENTIAL_CONTEXT")
            .unwrap()
            .1
            .unwrap()
            .to_owned();
        for name in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "CARGO_REGISTRY_TOKEN",
            "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
        ] {
            assert!(
                command
                    .get_envs()
                    .any(|(configured, value)| configured == name && value.is_none())
            );
        }
        let request = json!({
            "v":1,"kind":"get","operation":"publish","name":"library","vers":"1.0.0",
            "cksum":"b".repeat(64),
            "registry":{"index-url":"https://github.com/rust-lang/crates.io-index"}
        })
        .to_string();
        let preflight = json!({
            "v":1,"kind":"get","operation":"read","args":[],
            "registry":{"index-url":"https://github.com/rust-lang/crates.io-index"}
        })
        .to_string();
        assert_rejected_protocol_requests(&PathBuf::from(&context), &service);
        repository.write("src/lib.rs", b"pub fn changed() {}\n");
        for request in [&preflight, &request] {
            let mut rejected = Vec::new();
            serve_credential(
                &PathBuf::from(&context),
                &mut Cursor::new(request.as_bytes()),
                &mut rejected,
                &crp_publication::PublicationOutput::new(
                    "1.2.3",
                    false,
                    std::sync::Arc::new(crp_diag::Discard),
                ),
            )
            .unwrap_err();
            assert!(service.operations().is_empty());
            assert_eq!(
                serde_json::from_slice::<Value>(&rejected).unwrap(),
                json!({"v":[1]})
            );
        }
        repository.write("src/lib.rs", b"pub fn ready() {}\n");
        for request in [&preflight, &request] {
            let mut output = Vec::new();
            serve_credential(
                &PathBuf::from(&context),
                &mut Cursor::new(request.as_bytes()),
                &mut output,
                &crp_publication::PublicationOutput::new(
                    "1.2.3",
                    false,
                    std::sync::Arc::new(crp_diag::Discard),
                ),
            )
            .unwrap();
            let output = String::from_utf8(output).unwrap();
            let messages: Vec<Value> = output
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(messages.len(), 2);
            assert_eq!(messages.first().unwrap().get("v").unwrap(), &json!([1]));
            let credential = messages.last().unwrap().get("Ok").unwrap();
            assert_eq!(
                credential.get("token").unwrap(),
                "registry-credential-canary"
            );
            assert_eq!(credential.get("cache").unwrap(), "never");
            assert_eq!(credential.get("operation_independent").unwrap(), false);
        }
        assert_eq!(
            service.operations(),
            ["identity", "exchange", "identity", "exchange"]
        );
        let result = session.finish();
        assert_eq!(result.is_ok(), !reject_revocation);
        assert_eq!(
            service.operations(),
            [
                "identity", "exchange", "identity", "exchange", "revoke", "revoke"
            ]
        );
        assert!(!PathBuf::from(context).exists());
    }
}

fn assert_rejected_protocol_requests(context: &Path, service: &IdentityService) {
    // Unsupported operations and incomplete uploads must not acquire authority or leases.
    for (fields, kind) in [
        (
            json!({"kind":"get","operation":"yank"}),
            "operation-not-supported",
        ),
        (json!({"kind":"login"}), "operation-not-supported"),
        (json!({"kind":"logout"}), "operation-not-supported"),
        (json!({"kind":"get","operation":"publish"}), "other"),
        (json!({"v":2,"kind":"get","operation":"read"}), "other"),
        (
            json!({"kind":"get","operation":"read",
            "registry":{"index-url":"https://another.invalid/index"}}),
            "url-not-supported",
        ),
    ] {
        let mut input = json!({
            "v":1,"registry":{"index-url":"https://github.com/rust-lang/crates.io-index"}
        });
        input
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let mut output = Vec::new();
        serve_credential(
            context,
            &mut Cursor::new(input.to_string()),
            &mut output,
            &crp_publication::PublicationOutput::new(
                "1.2.3",
                false,
                std::sync::Arc::new(crp_diag::Discard),
            ),
        )
        .unwrap();
        let messages: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages.first().unwrap(), &json!({"v":[1]}));
        assert_eq!(messages.last().unwrap().pointer("/Err/kind").unwrap(), kind);
        assert!(messages.last().unwrap().get("Ok").is_none());
        assert!(service.operations().is_empty());
        assert_eq!(fs::read_dir(context.parent().unwrap()).unwrap().count(), 1);
    }
}

#[test]
#[cfg_attr(miri, ignore = "Uses owned credential files, Git and loopback HTTP")]
fn every_upload_gets_a_fresh_lease_and_cleanup_continues_after_revocation_failure() {
    for reject_first in [false, true] {
        let service = LeaseService::new(reject_first);
        let (_repository, session, context, mut issued) = issued_leases(&service);
        let result = session.finish();
        assert_eq!(result.is_ok(), !reject_first);
        let (assertions, tokens, mut revoked) = service.observations();
        assert_eq!(assertions, 2);
        assert_eq!(tokens, issued);
        revoked.sort();
        issued.sort();
        assert_eq!(revoked, issued);
        assert!(!context.parent().unwrap().exists());
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Uses owned partial lease files and a local revocation service"
)]
fn cleanup_revokes_valid_leases_and_retains_partial_state_failures() {
    let service = LeaseService::new(false);
    let (_repository, session, context, mut issued) = issued_leases(&service);
    let directory = context.parent().unwrap();
    // Distinct failures make continued error collection observable regardless of entry order.
    fs::write(directory.join("partial-token"), [0xff]).unwrap();
    fs::create_dir_all(directory.join("partial-directory")).unwrap();

    let error = session.finish().unwrap_err();
    let diagnostic = error.to_string();
    // Preserve the operation categories and inputs, not platform-specific I/O error wording.
    assert!(diagnostic.contains("read"));
    assert!(diagnostic.contains("write"));
    assert!(diagnostic.contains("partial-token"));
    assert!(diagnostic.contains("partial-directory"));
    let (assertions, tokens, mut revoked) = service.observations();
    assert_eq!(assertions, 2);
    assert_eq!(tokens, issued);
    revoked.sort();
    issued.sort();
    assert_eq!(revoked, issued);
    assert!(!directory.exists());
}

fn issued_leases(service: &LeaseService) -> (Repository, CredentialSession, PathBuf, Vec<String>) {
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
        b"schema-version=1\nrepository='example/library'\nrelease-branch='main'\ntargets=[]\n",
    );
    repository.command(&["add", "."]);
    repository.command(&["commit", "--quiet", "-m", "credential source"]);
    let source = repository.command(&["rev-parse", "HEAD"]).trim().to_owned();
    let publication = PublicationManifest::new(serde_json::from_value(json!({
            "schema_version":1,"tool_version":"1.0.0","source":source,
            "workspace_manifest":"Cargo.toml","config_path":".cargo/release_plan.toml",
            "configuration":{"schema-version":1,"repository":"example/library","release-branch":"main","targets":[]},
            "packages":(["alpha","beta"].map(|name| json!({"name":name,"version":"1.0.0","manifest":format!("{name}/Cargo.toml"),"binary":null})))
        })).unwrap()).unwrap();
    let diagnostics = crp_publication::PublicationOutput::new(
        "1.0.0",
        false,
        std::sync::Arc::new(crp_diag::Discard),
    );
    let session = CredentialSession::new(
        serde_json::from_value(
            json!({"request_url":format!("{}/identity",service.url()),"request_token":"fixture"}),
        )
        .unwrap(),
        publication,
        repository.path().join("Cargo.toml"),
        TrustedPublisher::with_endpoint(&format!("{}/tokens", service.url()), diagnostics.clone())
            .unwrap(),
    )
    .unwrap();
    let mut command = Command::new("cargo");
    session
        .configure(&mut command, Path::new("provider"))
        .unwrap();
    let context = PathBuf::from(
        command
            .get_envs()
            .find(|(name, _)| *name == "CARGO_RELEASE_PLAN_CREDENTIAL_CONTEXT")
            .unwrap()
            .1
            .unwrap(),
    );
    let mut issued = Vec::new();
    for name in ["alpha", "beta"] {
        let request = json!({"v":1,"kind":"get","operation":"publish","name":name,"vers":"1.0.0",
                "cksum":"b".repeat(64),"registry":{"index-url":"sparse+https://index.crates.io/"}});
        let mut output = Vec::new();
        serve_credential(
            &context,
            &mut Cursor::new(request.to_string()),
            &mut output,
            &diagnostics,
        )
        .unwrap();
        let output = String::from_utf8(output).unwrap();
        let response: Value = serde_json::from_str(output.lines().last().unwrap()).unwrap();
        issued.push(
            response
                .pointer("/Ok/token")
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    assert_ne!(issued.first(), issued.last());
    (repository, session, context, issued)
}
