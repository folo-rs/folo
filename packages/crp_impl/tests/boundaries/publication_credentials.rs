//! Full private credential protocol and lease cleanup against an isolated identity service.

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crp_impl::publication::credentials::{CredentialSession, serve_credential};
use crp_impl::publication::identity::{ActionsIdentity, TrustedPublisher};
use crp_impl::publication::manifest::{Publication, PublicationManifest};
use serde_json::{Value, json};

use crate::git_fixture::Repository;
use crate::publication_identity::IdentityService;

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
    let service = IdentityService::new(false);
    let repository = Repository::new();
    repository.write(
        "Cargo.toml",
        b"[package]\nname='library'\nversion='1.0.0'\nedition='2024'\n",
    );
    repository.write("src/lib.rs", b"pub fn ready() {}\n");
    repository.command(&["add", "."]);
    repository.command(&["commit", "--quiet", "-m", "credential source"]);
    let source = repository.command(&["rev-parse", "HEAD"]).trim().to_owned();
    let publication: Publication = serde_json::from_value(json!({
        "schema_version":1, "tool_version":"1.0.0", "source":source,
        "workspace_manifest":"Cargo.toml", "config_path":".cargo/release_plan.toml",
        "configuration":{"schema-version":1,"repository":"example/library","release-branch":"main","targets":[]},
        "packages":[{"name":"library","version":"1.0.0","manifest":"Cargo.toml","binary":null}]
    }))
    .unwrap();
    let identity: ActionsIdentity = serde_json::from_value(json!({
        "request_url":format!("{}/identity",service.url()),
        "request_token":"identity-credential-canary"
    }))
    .unwrap();
    let publisher = TrustedPublisher::with_endpoint(&format!("{}/tokens", service.url())).unwrap();
    let session = CredentialSession::new(
        identity,
        PublicationManifest::new(publication).unwrap(),
        repository.path().join("Cargo.toml"),
        repository.path().join("target"),
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
    let mut output = Vec::new();
    serve_credential(
        &PathBuf::from(&context),
        &mut Cursor::new(request.as_bytes()),
        &mut output,
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
    assert_eq!(service.operations(), ["identity", "exchange"]);
    session.finish().unwrap();
    assert_eq!(service.operations(), ["identity", "exchange", "revoke"]);
    assert!(!PathBuf::from(context).exists());
}
