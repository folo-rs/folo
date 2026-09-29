//! Invocation-owned credential state shared with Cargo's short-lived provider processes.

use std::ffi::{OsStr, OsString};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

use crp_workspace::command::BUILD_CREDENTIAL_VARIABLES;
use ohno::AppError;
use serde::{Deserialize, Serialize};
use tempfile::{Builder, NamedTempFile, TempDir};

use crate::publication::candidate::Repository;
use crate::publication::identity::{ActionsIdentity, TrustedPublisher};
use crate::publication::manifest::PublicationManifest;
use crate::{PublicationOutput, ReadFileError, WriteFileError};

/// Owns credential files through Cargo execution and revocation cleanup.
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
        let executable = executable
            .to_str()
            .ok_or_else(|| ProviderConfigurationError::new("executable path must be UTF-8"))?;
        let provider = serde_json::to_string(&[executable])?;
        command
            .arg("--config")
            .arg(format!("registry.credential-provider={provider}"))
            .env(CONTEXT_ENV, self.directory.path().join(CONTEXT_FILE));
        strip_credentials(command, env::vars_os().map(|(name, _)| name));
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
                Err(CredentialCleanupFailed::caused_by(cleanup, error).into())
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
    token_endpoint: String,
}

/// Cargo's request envelope, shared by credential lookups and account operations.
#[derive(Debug, Deserialize)]
struct CredentialRequest {
    v: u32,
    registry: CredentialRegistry,
    #[serde(flatten)]
    action: CredentialAction,
}

/// Only credential lookups can acquire publication authority.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum CredentialAction {
    Get {
        #[serde(flatten)]
        operation: CredentialOperation,
    },
    #[serde(other)]
    Unsupported,
}

/// Package identity is required for uploads, not for Cargo's preceding index reads.
#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "kebab-case")]
enum CredentialOperation {
    Read,
    Publish(PublishRequest),
    #[serde(other)]
    Unsupported,
}

/// Identifies the exact archive for which Cargo requests an upload credential.
#[derive(Debug, Deserialize)]
struct PublishRequest {
    name: String,
    #[serde(rename = "vers")]
    version: String,
    #[serde(rename = "cksum")]
    checksum: String,
}

/// Cargo's wire-level failures distinguish unsupported operations from invalid requests.
#[derive(Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum CredentialFailure {
    OperationNotSupported,
    UrlNotSupported,
    Other { message: &'static str },
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
// Cargo selects requests from the provider's advertised versions.
// Ref: https://doc.rust-lang.org/cargo/reference/credential-provider-protocol.html
const CREDENTIAL_PROTOCOL_VERSION: u32 = 1;

fn strip_credentials(command: &mut Command, names: impl Iterator<Item = OsString>) {
    for name in BUILD_CREDENTIAL_VARIABLES {
        command.env_remove(name);
    }
    for name in names {
        if registry_token(&name) {
            command.env_remove(name);
        }
    }
}

fn registry_token(name: &OsStr) -> bool {
    let name = name.to_string_lossy();
    #[cfg(windows)]
    let name = name.to_ascii_uppercase();
    name.starts_with("CARGO_REGISTRIES_") && name.ends_with("_TOKEN")
}

/// Serves one Cargo request; token output belongs only to the private credential-protocol pipe.
pub fn provide(
    input: &mut impl BufRead,
    output: &mut impl Write,
    diagnostics: &PublicationOutput,
) -> Result<(), AppError> {
    let path = env::var_os(CONTEXT_ENV).map(PathBuf::from).ok_or_else(|| {
        ProviderConfigurationError::new("provider requires its publication session")
    })?;
    serve_credential(&path, input, output, diagnostics)
}

/// Serves Cargo's credential protocol over supplied streams for boundary tests.
pub fn serve_credential(
    path: &Path,
    input: &mut impl BufRead,
    output: &mut impl Write,
    diagnostics: &PublicationOutput,
) -> Result<(), AppError> {
    let context: CredentialContext = serde_json::from_slice(
        &fs::read(path).map_err(|error| ReadFileError::caused_by(path, error))?,
    )
    .map_err(CredentialStateError::caused_by)?;
    context.publication.validate()?;
    serde_json::to_writer(
        &mut *output,
        &serde_json::json!({"v":[CREDENTIAL_PROTOCOL_VERSION]}),
    )?;
    writeln!(output)?;
    output.flush()?;
    let mut line = String::new();
    input.read_line(&mut line)?;
    if let Err(failure) = validate_request(&line, &context.publication) {
        serde_json::to_writer(&mut *output, &serde_json::json!({"Err": failure}))?;
        writeln!(output)?;
        output.flush()?;
        return Ok(());
    }
    let repository =
        Repository::discover(&context.manifest, &context.publication.publication.source)?;
    let publisher = TrustedPublisher::with_endpoint(&context.token_endpoint, diagnostics.clone())?;
    repository.ensure_clean_head()?;
    let token = publisher.exchange(&context.identity)?;
    let directory = path
        .parent()
        .expect("session context has an owning directory");
    if let Err(error) = save_lease(directory, &token) {
        if let Err(revocation) = publisher.revoke(&token) {
            diagnostics.best_effort_line(format_args!("{revocation}"));
        }
        return Err(error);
    }
    // Every upload must pass exact-request validation and acquire its own revocable lease.
    // Cargo must not reuse this token for another operation or later request.
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

fn validate_request(line: &str, manifest: &PublicationManifest) -> Result<(), CredentialFailure> {
    // Account-operation input can contain credentials. Do not echo deserializer values.
    let request: CredentialRequest =
        serde_json::from_str(line).map_err(|_sensitive_input| CredentialFailure::Other {
            message: "cannot decode Cargo's credential request",
        })?;
    if request.v != CREDENTIAL_PROTOCOL_VERSION {
        return Err(CredentialFailure::Other {
            message: "unsupported Cargo credential protocol version",
        });
    }
    if !matches!(
        request.registry.index_url.as_str(),
        "https://github.com/rust-lang/crates.io-index" | "sparse+https://index.crates.io/"
    ) {
        return Err(CredentialFailure::UrlNotSupported);
    }
    let request = match request.action {
        CredentialAction::Get {
            operation: CredentialOperation::Publish(request),
        } => request,
        CredentialAction::Get {
            operation: CredentialOperation::Read,
        } => {
            // Cargo requires a credential preflight even for the public crates.io index.
            // The same session/source checks and uncached lease lifecycle apply; uploads
            // still require exact package identity and acquire independent credentials.
            // Ref: cargo-release-plan/docs/implementation.md, Registry publication boundaries.
            return Ok(());
        }
        CredentialAction::Get {
            operation: CredentialOperation::Unsupported,
        }
        | CredentialAction::Unsupported => return Err(CredentialFailure::OperationNotSupported),
    };
    if !manifest
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
        return Err(CredentialFailure::Other {
            message: "credential request does not identify a requested crates.io publication",
        });
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
    let mut failure = None;
    for entry in fs::read_dir(directory).map_err(CredentialStateError::caused_by)? {
        let path = match entry {
            Ok(entry) => entry.path(),
            Err(error) => {
                record_cleanup_failure(
                    &mut failure,
                    CredentialStateError::caused_by(error).into(),
                    &publisher.output,
                );
                continue;
            }
        };
        if path.file_name().is_some_and(|name| name == CONTEXT_FILE) {
            continue;
        }
        match fs::read_to_string(&path) {
            Ok(token) => {
                if let Err(error) = publisher.revoke(&token) {
                    record_cleanup_failure(&mut failure, error, &publisher.output);
                }
            }
            Err(error) => {
                record_cleanup_failure(
                    &mut failure,
                    ReadFileError::caused_by(&path, error).into(),
                    &publisher.output,
                );
            }
        }
        if let Err(error) = fs::remove_file(&path) {
            record_cleanup_failure(
                &mut failure,
                WriteFileError::caused_by(&path, error).into(),
                &publisher.output,
            );
        }
    }
    failure.map_or(Ok(()), Err)
}

fn record_cleanup_failure(
    failure: &mut Option<AppError>,
    error: AppError,
    output: &PublicationOutput,
) {
    output.best_effort_line(format_args!("{error}"));
    *failure = Some(match failure.take() {
        Some(previous) => CredentialCleanupFailed::caused_by(error, previous).into(),
        None => error,
    });
}

#[ohno::error]
#[display("temporary publication credential state could not be maintained")]
struct CredentialStateError;

#[ohno::error]
#[display("credential provider configuration is invalid: {reason}")]
struct ProviderConfigurationError {
    reason: &'static str,
}

#[ohno::error]
#[display("credential cleanup also failed: {cleanup}")]
struct CredentialCleanupFailed {
    cleanup: AppError,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::publication::manifest::{InvalidManifest, Publication};

    #[test]
    fn registry_token_spelling_follows_the_platform_environment() {
        assert!(registry_token(OsStr::new("CARGO_REGISTRIES_PRIVATE_TOKEN")));
        assert_eq!(
            registry_token(OsStr::new("Cargo_Registries_Private_Token")),
            cfg!(windows)
        );
        assert!(!registry_token(OsStr::new(
            "CARGO_REGISTRIES_PRIVATE_INDEX"
        )));
    }

    #[test]
    fn cleanup_accumulation_retains_independent_failure_causes() {
        let output = PublicationOutput::new("1.0.0", false, std::sync::Arc::new(crp_diag::Discard));
        let mut failure = None;
        record_cleanup_failure(&mut failure, CredentialStateError::new().into(), &output);
        record_cleanup_failure(
            &mut failure,
            InvalidManifest::new("fixture".to_owned()).into(),
            &output,
        );
        let error = failure.unwrap();
        assert!(error.find_source::<CredentialStateError>().is_some());
        assert!(
            error
                .find_source::<CredentialCleanupFailed>()
                .unwrap()
                .cleanup
                .find_source::<InvalidManifest>()
                .is_some()
        );
    }

    #[test]
    fn cargo_receives_no_publication_credentials() {
        let mut command = Command::new("cargo");
        strip_credentials(
            &mut command,
            [
                "CARGO_REGISTRIES_PRIVATE_TOKEN",
                "CARGO_REGISTRIES_PRIVATE_INDEX",
                "UNRELATED",
            ]
            .into_iter()
            .map(OsString::from),
        );
        let removed: Vec<_> = command
            .get_envs()
            .filter_map(|(name, value)| value.is_none().then_some(name.to_owned()))
            .collect();
        for name in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GIT_TOKEN",
            "INPUT_TOKEN",
            "DEFAULT_GITHUB_TOKEN",
            "CARGO_REGISTRY_TOKEN",
            "ACTIONS_ID_TOKEN_REQUEST_URL",
            "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
            "CARGO_REGISTRIES_PRIVATE_TOKEN",
        ] {
            assert!(removed.contains(&OsString::from(name)));
        }
        for name in ["UNRELATED", "CARGO_REGISTRIES_PRIVATE_INDEX"] {
            assert!(!removed.contains(&OsString::from(name)));
        }
    }

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

    fn request() -> Value {
        json!({
            "v":1, "kind":"get", "operation":"publish",
            "name":"library", "vers":"1.0.0", "cksum":"b".repeat(64),
            "registry":{"index-url":"https://github.com/rust-lang/crates.io-index"}
        })
    }

    #[test]
    fn only_the_exact_registry_publication_request_can_acquire_a_token() {
        let publication = publication();
        validate_request(&request().to_string(), &publication).unwrap();
        let mut requested = request();
        *requested.pointer_mut("/registry/index-url").unwrap() =
            json!("sparse+https://index.crates.io/");
        *requested.get_mut("cksum").unwrap() = json!("B".repeat(64));
        validate_request(&requested.to_string(), &publication).unwrap();
        for (field, value) in [
            ("vers", json!("1.0.1")),
            ("name", json!("another")),
            ("v", json!(2)),
            ("cksum", json!("unknown")),
            ("cksum", json!("g".repeat(64))),
        ] {
            let mut requested = request();
            *requested.get_mut(field).unwrap() = value;
            assert!(matches!(
                validate_request(&requested.to_string(), &publication),
                Err(CredentialFailure::Other { .. })
            ));
        }
        for field in [
            "name",
            "vers",
            "cksum",
            "kind",
            "operation",
            "v",
            "registry",
        ] {
            let mut requested = request();
            requested.as_object_mut().unwrap().remove(field);
            assert!(matches!(
                validate_request(&requested.to_string(), &publication),
                Err(CredentialFailure::Other { .. })
            ));
        }
        assert!(matches!(
            validate_request("{", &publication),
            Err(CredentialFailure::Other { .. })
        ));
    }

    #[test]
    fn reads_and_unsupported_operations_do_not_require_publication_fields() {
        let publication = publication();
        let mut requested = json!({
            "v":1, "kind":"get", "operation":"read",
            "registry":{"index-url":"https://github.com/rust-lang/crates.io-index"}, "args":[]
        });
        validate_request(&requested.to_string(), &publication).unwrap();
        *requested.pointer_mut("/registry/index-url").unwrap() =
            json!("https://another.invalid/index");
        assert_eq!(
            validate_request(&requested.to_string(), &publication),
            Err(CredentialFailure::UrlNotSupported)
        );
        *requested.pointer_mut("/registry/index-url").unwrap() =
            json!("sparse+https://index.crates.io/");
        for operation in ["yank", "unyank", "owners", "future-operation"] {
            *requested.get_mut("operation").unwrap() = json!(operation);
            assert_eq!(
                validate_request(&requested.to_string(), &publication),
                Err(CredentialFailure::OperationNotSupported)
            );
        }
        requested.as_object_mut().unwrap().remove("operation");
        for kind in ["login", "logout", "future-kind"] {
            *requested.get_mut("kind").unwrap() = json!(kind);
            assert_eq!(
                validate_request(&requested.to_string(), &publication),
                Err(CredentialFailure::OperationNotSupported)
            );
        }
    }
}
