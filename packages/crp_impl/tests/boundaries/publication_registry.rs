//! Exact-version registry observations against local response fixtures.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::io::{self, Cursor};
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt as _;
#[cfg(windows)]
use std::os::windows::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crp_impl::publication::credentials::{CredentialSession, serve_credential};
use crp_impl::publication::identity::{ActionsIdentity, TrustedPublisher};
use crp_impl::publication::manifest::PublicationManifest;
use crp_impl::publication::registry::{
    RegistryClient, RegistryOutcome, RegistryPackage, RegistryRuntime, RegistryState, execute_with,
};
use crp_impl::verbose::Verbose;
use ohno::AppError;
use serde_json::json;
use tiny_http::{Response, StatusCode};

use crate::git_fixture::Repository;
use crate::http_fixture::HttpService;
use crate::publication_identity::IdentityService;

#[test]
#[cfg_attr(miri, ignore = "Uses a loopback HTTP service")]
fn absence_is_distinct_from_query_failure_or_mismatched_identity() {
    let service = HttpService::new(|_, request| {
        let (status, body) = match request.url() {
            "/index/pr/es/present" => (200, r#"{"name":"present","vers":"1.0.0","yanked":true}"#),
            "/index/mi/ss/missing" => (404, "{}"),
            "/index/fa/il/failed" => (503, "{}"),
            "/index/wr/on/wrong" => (200, r#"{"name":"another","vers":"1.0.0"}"#),
            "/index/br/ok/broken" => (200, "{}"),
            path => panic!("Unexpected fixture request: {path}"),
        };
        request
            .respond(Response::from_string(body).with_status_code(StatusCode(status)))
            .unwrap();
    });
    let client = RegistryClient::with_endpoint(&format!("{}/index", service.url())).unwrap();
    assert!(client.contains("present", "1.0.0").unwrap());
    assert!(!client.contains("present", "1.0.1").unwrap());
    assert!(!client.contains("missing", "1.0.0").unwrap());
    for name in ["failed", "wrong", "broken"] {
        client
            .contains_with_wait(name, "1.0.0", |_| {})
            .unwrap_err();
    }
    let invalid = RegistryClient::with_endpoint("not a registry URL").unwrap();
    let mut pauses = 0_usize;
    invalid
        .contains_with_wait("library", "1.0.0", |_| {
            pauses = pauses.checked_add(1).unwrap();
        })
        .unwrap_err();
    assert_eq!(pauses, 2);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Uses Git, Cargo metadata, credential files and loopback services"
)]
fn registry_orchestration_retains_completed_work_and_reconciles_upload_results() {
    for mode in [
        UploadMode::Success,
        UploadMode::PartialFailure,
        UploadMode::CompletedDespiteFailure,
        UploadMode::SpawnFailure,
        UploadMode::SourceChanged,
    ] {
        let (repository, publication) = publication_source();
        let available = Arc::new(Mutex::new(BTreeSet::from(["alpha".to_owned()])));
        let service = registry_service(&available);
        let client = RegistryClient::with_endpoint(&format!("{}/index", service.url())).unwrap();
        let identity = IdentityService::new(false);
        let runtime = UploadRuntime {
            repository: &repository,
            identity: &identity,
            available,
            mode,
            uploads: Cell::new(0),
            pauses: Cell::new(0),
            context: RefCell::new(None),
            target: RefCell::new(None),
        };
        let mut outcome = outcome(&publication, false);
        let result = execute_with(
            &publication,
            &repository.path().join("Cargo.toml"),
            &client,
            &mut outcome,
            Verbose::new(true),
            &runtime,
        );
        assert_eq!(
            result.is_ok(),
            matches!(
                mode,
                UploadMode::Success | UploadMode::CompletedDespiteFailure
            )
        );
        assert_eq!(
            outcome.packages.first().unwrap().state,
            RegistryState::AlreadyPresent
        );
        assert_eq!(
            outcome.packages.last().unwrap().state,
            if matches!(mode, UploadMode::PartialFailure | UploadMode::SpawnFailure) {
                RegistryState::Missing
            } else {
                RegistryState::Published
            }
        );
        assert_eq!(
            outcome.notes.is_empty(),
            !matches!(mode, UploadMode::CompletedDespiteFailure)
        );
        assert_eq!(runtime.uploads.get(), 1);
        assert_eq!(
            runtime.pauses.get(),
            if matches!(mode, UploadMode::PartialFailure) {
                5
            } else {
                0
            }
        );
        assert!(!runtime.context.borrow().as_ref().unwrap().exists());
        assert!(!runtime.target.borrow().as_ref().unwrap().exists());
        assert_eq!(
            identity.operations(),
            if matches!(mode, UploadMode::SpawnFailure) {
                Vec::new()
            } else {
                vec!["identity", "exchange", "revoke"]
            }
        );
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Uses real source verification and loopback registry observations"
)]
fn established_versions_and_dry_runs_never_acquire_upload_credentials() {
    for dry_run in [false, true] {
        let (repository, publication) = publication_source();
        let available = Arc::new(Mutex::new(BTreeSet::from(["alpha".to_owned()])));
        if !dry_run {
            available.lock().unwrap().insert("beta".to_owned());
        }
        let service = registry_service(&available);
        let client = RegistryClient::with_endpoint(&format!("{}/index", service.url())).unwrap();
        let identity = IdentityService::new(false);
        let runtime = UploadRuntime {
            repository: &repository,
            identity: &identity,
            available,
            mode: UploadMode::Success,
            uploads: Cell::new(0),
            pauses: Cell::new(0),
            context: RefCell::new(None),
            target: RefCell::new(None),
        };
        let mut outcome = outcome(&publication, dry_run);
        execute_with(
            &publication,
            &repository.path().join("Cargo.toml"),
            &client,
            &mut outcome,
            Verbose::new(true),
            &runtime,
        )
        .unwrap();
        assert_eq!(
            outcome.packages.first().unwrap().state,
            RegistryState::AlreadyPresent
        );
        assert_eq!(
            outcome.packages.last().unwrap().state,
            if dry_run {
                RegistryState::WouldPublish
            } else {
                RegistryState::AlreadyPresent
            }
        );
        assert_eq!(runtime.uploads.get(), 0);
        assert!(runtime.context.borrow().is_none());
        assert!(runtime.target.borrow().is_none());
        assert!(identity.operations().is_empty());
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Checks publication requests against real committed source"
)]
fn mismatched_source_configuration_and_requests_fail_before_registry_or_credentials() {
    let (repository, publication) = publication_source();
    let service =
        HttpService::new(|_, _| panic!("source validation must precede registry queries"));
    let client = RegistryClient::with_endpoint(&format!("{}/index", service.url())).unwrap();
    let identity = IdentityService::new(false);
    let runtime = UploadRuntime {
        repository: &repository,
        identity: &identity,
        available: Arc::new(Mutex::new(BTreeSet::new())),
        mode: UploadMode::Success,
        uploads: Cell::new(0),
        pauses: Cell::new(0),
        context: RefCell::new(None),
        target: RefCell::new(None),
    };
    for wrong_configuration in [false, true] {
        let mut value = serde_json::to_value(&publication.publication).unwrap();
        if wrong_configuration {
            _ = value
                .get_mut("configuration")
                .unwrap()
                .as_object_mut()
                .unwrap()
                .insert("repository".to_owned(), json!("example/different"));
        } else {
            value
                .get_mut("packages")
                .unwrap()
                .as_array_mut()
                .unwrap()
                .clear();
        }
        let altered = PublicationManifest::new(serde_json::from_value(value).unwrap()).unwrap();
        execute_with(
            &altered,
            &repository.path().join("Cargo.toml"),
            &client,
            &mut outcome(&altered, false),
            Verbose::new(true),
            &runtime,
        )
        .unwrap_err();
    }
    repository.write(
        "separate/Cargo.toml",
        b"[workspace]\n[package]\nname='separate'\nversion='1.0.0'\nedition='2024'\n",
    );
    repository.write("separate/src/lib.rs", b"pub fn separate() {}\n");
    repository.command(&["add", "."]);
    repository.command(&["commit", "--quiet", "-m", "separate Cargo workspace"]);
    let mut body = publication.publication;
    body.source = repository.command(&["rev-parse", "HEAD"]).trim().to_owned();
    let current = PublicationManifest::new(body).unwrap();
    execute_with(
        &current,
        &repository.path().join("separate/Cargo.toml"),
        &client,
        &mut outcome(&current, false),
        Verbose::new(true),
        &runtime,
    )
    .unwrap_err();
    assert_eq!(runtime.uploads.get(), 0);
    assert!(runtime.target.borrow().is_none());
    assert!(identity.operations().is_empty());
}

/// Observes the real configured command without executing a registry upload.
struct UploadRuntime<'a> {
    repository: &'a Repository,
    identity: &'a IdentityService,
    available: Arc<Mutex<BTreeSet<String>>>,
    mode: UploadMode,
    uploads: Cell<usize>,
    pauses: Cell<usize>,
    context: RefCell<Option<PathBuf>>,
    target: RefCell<Option<PathBuf>>,
}

impl RegistryRuntime for UploadRuntime<'_> {
    fn credentials(
        &self,
        publication: &PublicationManifest,
        manifest: &Path,
        target: &Path,
    ) -> Result<CredentialSession, AppError> {
        _ = self.target.replace(Some(target.to_owned()));
        CredentialSession::new(
            serde_json::from_value::<ActionsIdentity>(json!({
                "request_url":format!("{}/identity",self.identity.url()),
                "request_token":"identity-credential-canary"
            }))?,
            publication.clone(),
            manifest.to_owned(),
            target.to_owned(),
            TrustedPublisher::with_endpoint(&format!("{}/tokens", self.identity.url()))?,
        )
    }

    fn upload(&self, command: &mut Command) -> io::Result<Output> {
        self.uploads.set(self.uploads.get().checked_add(1).unwrap());
        assert_eq!(
            command.get_current_dir().unwrap().canonicalize().unwrap(),
            self.repository.path().canonicalize().unwrap()
        );
        let arguments = command.get_args().collect::<Vec<_>>();
        assert_eq!(
            arguments.get(..4).unwrap(),
            ["publish", "--registry", "crates-io", "--locked"]
        );
        let packages = arguments
            .windows(2)
            .filter_map(|pair| match pair {
                [argument, value] if *argument == "--package" => Some(*value),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(packages, ["beta"]);
        let context = PathBuf::from(
            command
                .get_envs()
                .find(|(name, _)| *name == "CARGO_RELEASE_PLAN_CREDENTIAL_CONTEXT")
                .unwrap()
                .1
                .unwrap(),
        );
        _ = self.context.replace(Some(context.clone()));
        if matches!(self.mode, UploadMode::SpawnFailure) {
            return Err(io::Error::from(io::ErrorKind::NotFound));
        }
        let request = json!({
            "v":1,"kind":"get","operation":"publish","name":"beta","vers":"1.0.0",
            "cksum":"b".repeat(64),
            "registry":{"index-url":"https://github.com/rust-lang/crates.io-index"}
        });
        serve_credential(
            &context,
            &mut Cursor::new(request.to_string()),
            &mut Vec::new(),
        )
        .unwrap();
        if !matches!(self.mode, UploadMode::PartialFailure) {
            self.available.lock().unwrap().insert("beta".to_owned());
        }
        if matches!(self.mode, UploadMode::SourceChanged) {
            self.repository
                .write("beta/src/lib.rs", b"pub fn changed() {}\n");
        }
        Ok(Output {
            // Supply a native successful or unsuccessful status without launching an upload.
            status: ExitStatus::from_raw(
                if matches!(
                    self.mode,
                    UploadMode::PartialFailure | UploadMode::CompletedDespiteFailure
                ) {
                    1
                } else {
                    0
                },
            ),
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
    }

    fn pause(&self, _delay: Duration) {
        self.pauses.set(self.pauses.get().checked_add(1).unwrap());
    }
}

/// Supplies independent process, registry and source observations after the attempted upload.
#[derive(Clone, Copy)]
enum UploadMode {
    Success,
    PartialFailure,
    CompletedDespiteFailure,
    SpawnFailure,
    SourceChanged,
}

fn registry_service(available: &Arc<Mutex<BTreeSet<String>>>) -> HttpService {
    HttpService::new({
        let available = Arc::clone(available);
        move |_, request| {
            let name = request.url().rsplit('/').next().unwrap();
            let (status, body) = if available.lock().unwrap().contains(name) {
                (200, json!({"name":name,"vers":"1.0.0"}))
            } else {
                (404, json!({}))
            };
            request
                .respond(
                    Response::from_string(body.to_string()).with_status_code(StatusCode(status)),
                )
                .unwrap();
        }
    })
}

fn publication_source() -> (Repository, PublicationManifest) {
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
        b"schema-version=1\nrepository='example/releases'\nrelease-branch='main'\ntargets=[]\n",
    );
    repository.command(&["add", "."]);
    repository.command(&["commit", "--quiet", "-m", "publication source"]);
    let publication = PublicationManifest::new(serde_json::from_value(json!({
            "schema_version":1,"tool_version":"1.0.0",
            "source":repository.command(&["rev-parse","HEAD"]).trim(),
            "workspace_manifest":"Cargo.toml","config_path":".cargo/release_plan.toml",
            "configuration":{"schema-version":1,"repository":"example/releases","release-branch":"main","targets":[]},
            "packages":[
                {"name":"alpha","version":"1.0.0","manifest":"alpha/Cargo.toml","binary":null},
                {"name":"beta","version":"1.0.0","manifest":"beta/Cargo.toml","binary":null}
            ]
        })).unwrap()).unwrap();
    (repository, publication)
}

fn outcome(publication: &PublicationManifest, dry_run: bool) -> RegistryOutcome {
    RegistryOutcome {
        schema_version: 1,
        publication_id: publication.id.clone(),
        phase: "registry".to_owned(),
        dry_run,
        complete: false,
        packages: publication
            .publication
            .packages
            .iter()
            .map(|package| RegistryPackage {
                name: package.name.clone(),
                version: package.version.clone(),
                state: RegistryState::Unknown,
            })
            .collect(),
        errors: Vec::new(),
        notes: Vec::new(),
        github: None,
    }
}
