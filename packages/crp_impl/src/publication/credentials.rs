//! Invocation-owned credential state shared with Cargo's short-lived provider processes.

use std::fmt::Write as _;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use ohno::AppError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::{Builder, NamedTempFile, TempDir};

use crate::publication::candidate::Repository;
use crate::publication::identity::{ActionsIdentity, TrustedPublisher};
use crate::publication::manifest::{InvalidManifest, PublicationManifest};
use crate::publication::resolution::verify_packaged_closure;
use crate::{ReadFileError, WriteFileError};

/// Owns temporary credential files until Cargo exits and every issued token is revoked.
///
/// Provider processes do not outlive Cargo and cannot retain leases in memory for their parent.
/// This directory is private runtime state, never source, an outcome or a workflow artifact.
#[derive(Debug)]
pub struct CredentialSession {
    directory: TempDir,
    publisher: TrustedPublisher,
}

impl CredentialSession {
    pub fn new(
        identity: ActionsIdentity,
        publication: PublicationManifest,
        manifest: PathBuf,
        target: PathBuf,
        publisher: TrustedPublisher,
    ) -> Result<Self, AppError> {
        publication.validate()?;
        let directory = Builder::new()
            .prefix("cargo-release-plan-credentials-")
            .tempdir()
            .map_err(CredentialStateError::caused_by)?;
        let context = CredentialContext {
            identity,
            publication,
            manifest,
            target,
            token_endpoint: publisher.endpoint().to_owned(),
        };
        let path = directory.path().join(CONTEXT_FILE);
        let mut file =
            NamedTempFile::new_in(directory.path()).map_err(CredentialStateError::caused_by)?;
        serde_json::to_writer(&mut file, &context).map_err(CredentialStateError::caused_by)?;
        file.persist_noclobber(&path)
            .map_err(CredentialStateError::caused_by)?;
        Ok(Self {
            directory,
            publisher,
        })
    }

    /// Routes only the Cargo credential protocol through the selected executable.
    pub fn configure(&self, command: &mut Command, executable: &Path) -> Result<(), AppError> {
        let executable = executable.to_str().ok_or_else(|| {
            InvalidManifest::new("credential provider executable path must be UTF-8".to_owned())
        })?;
        let provider = serde_json::to_string(&[executable])?;
        command
            .arg("--config")
            .arg(format!("registry.credential-provider={provider}"))
            .env(CONTEXT_ENV, self.directory.path().join(CONTEXT_FILE));
        for name in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GIT_TOKEN",
            "ACTIONS_ID_TOKEN_REQUEST_URL",
            "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
            "CARGO_REGISTRY_TOKEN",
        ] {
            command.env_remove(name);
        }
        for (name, _) in env::vars_os() {
            if name.to_string_lossy().starts_with("CARGO_REGISTRIES_")
                && name.to_string_lossy().ends_with("_TOKEN")
            {
                command.env_remove(name);
            }
        }
        Ok(())
    }

    /// Revokes all credentials, retaining failure diagnostics even when another revocation fails.
    pub fn finish(self) -> Result<(), AppError> {
        let result = revoke_directory(self.directory.path(), &self.publisher);
        let cleanup = self
            .directory
            .close()
            .map_err(CredentialStateError::caused_by);
        match (result, cleanup) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(error)) => Err(error.into()),
            (Err(error), Err(cleanup)) => {
                eprintln!("{cleanup}");
                Err(error)
            }
        }
    }
}

/// Sensitive identity and immutable request context for a single Cargo invocation.
#[derive(Deserialize, Serialize)]
struct CredentialContext {
    identity: ActionsIdentity,
    publication: PublicationManifest,
    manifest: PathBuf,
    target: PathBuf,
    token_endpoint: String,
}

/// Cargo's versioned request, including the exact package archive awaiting upload.
#[derive(Debug, Deserialize)]
struct CredentialRequest {
    v: u32,
    kind: String,
    operation: String,
    name: String,
    #[serde(rename = "vers")]
    version: String,
    #[serde(rename = "cksum")]
    checksum: String,
    registry: CredentialRegistry,
}

/// Registry identity supplied by Cargo, not a destination chosen by the provider.
#[derive(Debug, Deserialize)]
struct CredentialRegistry {
    #[serde(rename = "index-url")]
    index_url: String,
}

// Cargo invokes a fresh provider process per uncached request. Only the context location
// travels in the subprocess environment; identity values and issued tokens do not.
const CONTEXT_ENV: &str = "CARGO_RELEASE_PLAN_CREDENTIAL_CONTEXT";
const CONTEXT_FILE: &str = "context.json";
const LEASE_PREFIX: &str = "token-";
const CRATES_IO_SOURCE: &str = "registry+https://github.com/rust-lang/crates.io-index";

/// Serves one Cargo request; token output belongs only to the private credential-protocol pipe.
pub(crate) fn provide() -> Result<(), AppError> {
    let path = env::var_os(CONTEXT_ENV).map(PathBuf::from).ok_or_else(|| {
        InvalidManifest::new("credential provider requires its publication session".to_owned())
    })?;
    serve_credential(&path, &mut io::stdin().lock(), &mut io::stdout().lock())
}

/// Serves the same private provider protocol over supplied streams for native boundary tests.
pub fn serve_credential(
    path: &Path,
    input: &mut impl BufRead,
    output: &mut impl Write,
) -> Result<(), AppError> {
    let context: CredentialContext = serde_json::from_slice(
        &fs::read(path).map_err(|error| ReadFileError::caused_by(path, error))?,
    )
    .map_err(CredentialStateError::caused_by)?;
    context.publication.validate()?;
    writeln!(output, "{{\"v\":[1]}}")?;
    output.flush()?;
    let mut line = String::new();
    input.read_line(&mut line)?;
    let request: CredentialRequest =
        serde_json::from_str(&line).map_err(CredentialStateError::caused_by)?;
    validate_request(&request, &context.publication)?;
    let repository =
        Repository::discover(&context.manifest, &context.publication.publication.source)?;
    repository.ensure_clean_head()?;
    let package = context
        .publication
        .publication
        .packages
        .iter()
        .find(|package| package.name == request.name)
        .expect("request validation requires an exact publication package");
    if package.binary.is_some() {
        let archive = context
            .target
            .join("package")
            .join(format!("{}-{}.crate", request.name, request.version));
        let bytes =
            fs::read(&archive).map_err(|error| ReadFileError::caused_by(&archive, error))?;
        let mut checksum = String::new();
        for byte in Sha256::digest(&bytes) {
            write!(checksum, "{byte:02x}")?;
        }
        if checksum != request.checksum {
            return Err(InvalidManifest::new(
                "Cargo upload checksum differs from the inspected binary package archive"
                    .to_owned(),
            )
            .into());
        }
        verify_packaged_closure(
            &context.manifest,
            &bytes,
            &request.name,
            &request.version,
            CRATES_IO_SOURCE,
        )?;
    }
    repository.ensure_clean_head()?;
    let publisher = TrustedPublisher::with_endpoint(&context.token_endpoint)?;
    let token = publisher.exchange(&context.identity)?;
    let directory = path
        .parent()
        .expect("session context has an owning directory");
    if let Err(error) = save_lease(directory, &token) {
        if let Err(revocation) = publisher.revoke(&token) {
            eprintln!("{revocation}");
        }
        return Err(error);
    }
    serde_json::to_writer(
        &mut *output,
        &serde_json::json!({
            "Ok": {
                "kind": "get",
                "token": token,
                "cache": "never",
                "operation_independent": false
            }
        }),
    )?;
    writeln!(output)?;
    output.flush()?;
    Ok(())
}

fn validate_request(
    request: &CredentialRequest,
    manifest: &PublicationManifest,
) -> Result<(), AppError> {
    if request.v != 1
        || request.kind != "get"
        || request.operation != "publish"
        || !matches!(
            request.registry.index_url.as_str(),
            "https://github.com/rust-lang/crates.io-index" | "sparse+https://index.crates.io/"
        )
        || !manifest
            .publication
            .packages
            .iter()
            .any(|package| package.name == request.name && package.version == request.version)
        || request.checksum.len() != 64
        || !request
            .checksum
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(InvalidManifest::new(
            "credential request does not identify a requested crates.io publication".to_owned(),
        )
        .into());
    }
    Ok(())
}

fn save_lease(directory: &Path, token: &str) -> Result<(), AppError> {
    let mut lease = Builder::new()
        .prefix(LEASE_PREFIX)
        .tempfile_in(directory)
        .map_err(CredentialStateError::caused_by)?;
    lease
        .write_all(token.as_bytes())
        .map_err(CredentialStateError::caused_by)?;
    lease.keep().map_err(CredentialStateError::caused_by)?;
    Ok(())
}

fn revoke_directory(directory: &Path, publisher: &TrustedPublisher) -> Result<(), AppError> {
    let mut failed = false;
    for entry in fs::read_dir(directory).map_err(CredentialStateError::caused_by)? {
        let path = entry.map_err(CredentialStateError::caused_by)?.path();
        if path.file_name().is_some_and(|name| name == CONTEXT_FILE) {
            continue;
        }
        match fs::read_to_string(&path) {
            Ok(token) => {
                if let Err(error) = publisher.revoke(&token) {
                    eprintln!("{error}");
                    failed = true;
                }
            }
            Err(error) => {
                eprintln!("{}", ReadFileError::caused_by(&path, error));
                failed = true;
            }
        }
        if let Err(error) = fs::remove_file(&path) {
            eprintln!("{}", WriteFileError::caused_by(&path, error));
            failed = true;
        }
    }
    if failed {
        return Err(CredentialRevocationFailed::new().into());
    }
    Ok(())
}

#[ohno::error]
#[display("temporary publication credential state could not be maintained")]
struct CredentialStateError;

#[ohno::error]
#[display("one or more publication credentials could not be revoked; see diagnostics")]
struct CredentialRevocationFailed;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::publication::manifest::Publication;

    fn publication() -> PublicationManifest {
        let publication: Publication = serde_json::from_value(json!({
            "schema_version": 1, "tool_version": "1.0.0",
            "source": "a".repeat(40),
            "workspace_manifest": "Cargo.toml",
            "config_path": ".cargo/release_plan.toml",
            "configuration": {
                "schema-version": 1, "repository": "example/library",
                "release-branch": "main", "targets": []
            },
            "packages": [{"name":"library","version":"1.0.0","manifest":"library/Cargo.toml","binary":null}]
        }))
        .unwrap();
        PublicationManifest::new(publication).unwrap()
    }

    fn request() -> CredentialRequest {
        serde_json::from_value(json!({
            "v":1, "kind":"get", "operation":"publish",
            "name":"library", "vers":"1.0.0", "cksum":"b".repeat(64),
            "registry":{"index-url":"https://github.com/rust-lang/crates.io-index"}
        }))
        .unwrap()
    }

    #[test]
    fn only_the_exact_registry_publication_request_can_acquire_a_token() {
        let publication = publication();
        validate_request(&request(), &publication).unwrap();
        let mut requested = request();
        requested.registry.index_url = "sparse+https://index.crates.io/".to_owned();
        validate_request(&requested, &publication).unwrap();
        let mut requested = request();
        requested.version = "1.0.1".to_owned();
        validate_request(&requested, &publication).unwrap_err();
        let mut requested = request();
        requested.name = "another".to_owned();
        validate_request(&requested, &publication).unwrap_err();
        let mut requested = request();
        requested.registry.index_url = "https://another.invalid/index".to_owned();
        validate_request(&requested, &publication).unwrap_err();
        let mut requested = request();
        requested.operation = "read".to_owned();
        validate_request(&requested, &publication).unwrap_err();
        let mut requested = request();
        requested.kind = "login".to_owned();
        validate_request(&requested, &publication).unwrap_err();
        let mut requested = request();
        requested.v = 2;
        validate_request(&requested, &publication).unwrap_err();
        let mut requested = request();
        requested.checksum = "unknown".to_owned();
        validate_request(&requested, &publication).unwrap_err();
    }
}
