//! Full private credential protocol and lease cleanup against an isolated identity service.

use std::fmt::Write as _;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crp_impl::publication::credentials::{CredentialSession, serve_credential};
use crp_impl::publication::identity::{ActionsIdentity, TrustedPublisher};
use crp_impl::publication::manifest::{Publication, PublicationManifest};
use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tar::{Builder, Header};

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
    for reject_revocation in [false, true] {
        let service = IdentityService::with_failures(false, reject_revocation);
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
        let publisher =
            TrustedPublisher::with_endpoint(&format!("{}/tokens", service.url())).unwrap();
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
        let result = session.finish();
        assert_eq!(result.is_ok(), !reject_revocation);
        assert_eq!(service.operations(), ["identity", "exchange", "revoke"]);
        assert!(!PathBuf::from(context).exists());
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Uses Git, package archives, private credential files and loopback HTTP"
)]
fn binary_credentials_require_the_exact_archive_and_assessed_lockfile() {
    for case in [
        ArchiveCase::Missing,
        ArchiveCase::WrongChecksum,
        ArchiveCase::WrongResolution,
        ArchiveCase::Valid,
    ] {
        let service = IdentityService::new(false);
        let repository = Repository::new();
        repository.write(
            "Cargo.toml",
            b"[package]\nname='tool'\nversion='1.0.0'\nedition='2024'\n",
        );
        repository.write("src/main.rs", b"fn main() {}\n");
        let lock = "version=4\n[[package]]\nname='tool'\nversion='1.0.0'\n";
        repository.write("Cargo.lock", lock.as_bytes());
        repository.command(&["add", "."]);
        repository.command(&["commit", "--quiet", "-m", "binary credential source"]);
        let source = repository.command(&["rev-parse", "HEAD"]).trim().to_owned();
        let publication = PublicationManifest::new(serde_json::from_value(json!({
            "schema_version":1,"tool_version":"1.0.0","source":source,
            "workspace_manifest":"Cargo.toml","config_path":".cargo/release_plan.toml",
            "configuration":{"schema-version":1,"repository":"example/tool","release-branch":"main",
                "targets":["x86_64-unknown-linux-gnu"]},
            "packages":[{"name":"tool","version":"1.0.0","manifest":"Cargo.toml",
                "binary":{"name":"tool","targets":["x86_64-unknown-linux-gnu"]}}]
        })).unwrap()).unwrap();
        let target = tempfile::tempdir().unwrap();
        let archive_lock = if matches!(case, ArchiveCase::WrongResolution) {
            lock.replace("1.0.0", "1.0.1")
        } else {
            lock.to_owned()
        };
        let archive = package_archive(archive_lock.as_bytes());
        let mut checksum = String::new();
        for byte in Sha256::digest(&archive) {
            write!(checksum, "{byte:02x}").unwrap();
        }
        if matches!(case, ArchiveCase::WrongChecksum) {
            let replacement = if checksum.starts_with('a') { "b" } else { "a" };
            checksum.replace_range(..1, replacement);
        }
        if !matches!(case, ArchiveCase::Missing) {
            fs::create_dir_all(target.path().join("package")).unwrap();
            fs::write(target.path().join("package/tool-1.0.0.crate"), archive).unwrap();
        }
        let identity = serde_json::from_value(json!({
            "request_url":format!("{}/identity",service.url()),
            "request_token":"identity-credential-canary"
        }))
        .unwrap();
        let session = CredentialSession::new(
            identity,
            publication,
            repository.path().join("Cargo.toml"),
            target.path().to_owned(),
            TrustedPublisher::with_endpoint(&format!("{}/tokens", service.url())).unwrap(),
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
        let request = json!({
            "v":1,"kind":"get","operation":"publish","name":"tool","vers":"1.0.0",
            "cksum":checksum,"registry":{"index-url":"sparse+https://index.crates.io/"}
        });
        let mut output = Vec::new();
        let result = serve_credential(&context, &mut Cursor::new(request.to_string()), &mut output);
        assert_eq!(result.is_ok(), matches!(case, ArchiveCase::Valid));
        assert_eq!(
            service.operations(),
            if matches!(case, ArchiveCase::Valid) {
                vec!["identity", "exchange"]
            } else {
                Vec::new()
            }
        );
        session.finish().unwrap();
        assert!(!context.exists());
    }
}

/// Independent archive failures must reject credential issuance before any exchange.
#[derive(Clone, Copy)]
enum ArchiveCase {
    Missing,
    WrongChecksum,
    WrongResolution,
    Valid,
}

fn package_archive(lockfile: &[u8]) -> Vec<u8> {
    let mut archive = Builder::new(GzEncoder::new(Vec::new(), Compression::fast()));
    let mut header = Header::new_gnu();
    header.set_size(lockfile.len().try_into().unwrap());
    // A regular readable archive member, matching Cargo's package layout.
    header.set_mode(0o644);
    header.set_cksum();
    archive
        .append_data(&mut header, "tool-1.0.0/Cargo.lock", lockfile)
        .unwrap();
    archive.into_inner().unwrap().finish().unwrap()
}
