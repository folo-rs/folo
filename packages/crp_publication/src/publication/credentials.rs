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
    // Native credential tests inspect the actual child environment and Cargo provider selection.
    #[cfg_attr(test, mutants::skip)]
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
    // Real lease files and directory removal are covered by publication_credentials.
    #[cfg_attr(test, mutants::skip)]
    pub fn finish(self) -> Result<(), AppError> {
        let result = revoke_directory(self.directory.path(), &self.publisher);
        finish_cleanup(result, || {
            self.directory
                .close()
                .map_err(CredentialStateError::caused_by)
                .map_err(Into::into)
        })
    }
}

fn finish_cleanup(
    revocation: Result<(), AppError>,
    close: impl FnOnce() -> Result<(), AppError>,
) -> Result<(), AppError> {
    match (revocation, close()) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => {
            Err(CredentialCleanupFailed::caused_by(cleanup, error).into())
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
// The real Cargo boundary tests invoke the provider using its process environment.
#[cfg_attr(test, mutants::skip)]
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
// This adapter reads private context and rechecks actual Git source before HTTP issuance.
// publication_credentials covers those boundaries; serve_with and acquire_lease own the decisions.
#[cfg_attr(test, mutants::skip)]
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
    serve_with(&context.publication, input, output, || {
        let repository =
            Repository::discover(&context.manifest, &context.publication.publication.source)?;
        let publisher =
            TrustedPublisher::with_endpoint(&context.token_endpoint, diagnostics.clone())?;
        repository.ensure_clean_head()?;
        let directory = path
            .parent()
            .expect("session context has an owning directory");
        acquire_lease(
            || publisher.exchange(&context.identity),
            |token| save_lease(directory, token),
            |token| publisher.revoke(token),
            diagnostics,
        )
    })
}

fn serve_with(
    publication: &PublicationManifest,
    input: &mut impl BufRead,
    output: &mut impl Write,
    issue: impl FnOnce() -> Result<String, AppError>,
) -> Result<(), AppError> {
    publication.validate()?;
    serde_json::to_writer(
        &mut *output,
        &serde_json::json!({"v":[CREDENTIAL_PROTOCOL_VERSION]}),
    )?;
    writeln!(output)?;
    output.flush()?;
    let mut line = String::new();
    input.read_line(&mut line)?;
    if let Err(failure) = validate_request(&line, publication) {
        serde_json::to_writer(&mut *output, &serde_json::json!({"Err": failure}))?;
        writeln!(output)?;
        output.flush()?;
        return Ok(());
    }
    let token = issue()?;
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

fn acquire_lease(
    exchange: impl FnOnce() -> Result<String, AppError>,
    save: impl FnOnce(&str) -> Result<(), AppError>,
    revoke: impl FnOnce(&str) -> Result<(), AppError>,
    diagnostics: &PublicationOutput,
) -> Result<String, AppError> {
    let token = exchange()?;
    if let Err(error) = save(&token) {
        if let Err(revocation) = revoke(&token) {
            diagnostics.best_effort_line(format_args!("{revocation}"));
        }
        return Err(error);
    }
    Ok(token)
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

// Real private-file persistence is observed by the parent revoking every issued lease.
#[cfg_attr(test, mutants::skip)]
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

// Directory enumeration and file cleanup are covered by publication_credentials, including
// unreadable/partial entries. revoke_leases tests selection and independent failure accounting.
#[cfg_attr(test, mutants::skip)]
fn revoke_directory(directory: &Path, publisher: &TrustedPublisher) -> Result<(), AppError> {
    let entries = fs::read_dir(directory).map_err(CredentialStateError::caused_by)?;
    revoke_leases(
        entries.map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(CredentialStateError::caused_by)
                .map_err(Into::into)
        }),
        |path| {
            fs::read_to_string(path).map_err(|error| ReadFileError::caused_by(path, error).into())
        },
        |token| publisher.revoke(token),
        |path| fs::remove_file(path).map_err(|error| WriteFileError::caused_by(path, error).into()),
        &publisher.output,
    )
}

fn revoke_leases(
    entries: impl Iterator<Item = Result<PathBuf, AppError>>,
    mut read: impl FnMut(&Path) -> Result<String, AppError>,
    mut revoke: impl FnMut(&str) -> Result<(), AppError>,
    mut remove: impl FnMut(&Path) -> Result<(), AppError>,
    output: &PublicationOutput,
) -> Result<(), AppError> {
    let mut failure = None;
    for entry in entries {
        let path = match entry {
            Ok(path) => path,
            Err(error) => {
                record_cleanup_failure(&mut failure, error, output);
                continue;
            }
        };
        if path.file_name().is_some_and(|name| name == CONTEXT_FILE) {
            continue;
        }
        match read(&path) {
            Ok(token) => {
                if let Err(error) = revoke(&token) {
                    record_cleanup_failure(&mut failure, error, output);
                }
            }
            Err(error) => {
                record_cleanup_failure(&mut failure, error, output);
            }
        }
        if let Err(error) = remove(&path) {
            record_cleanup_failure(&mut failure, error, output);
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
    use std::cell::Cell;
    use std::io::{self, Cursor};
    use std::sync::{Arc, Mutex};

    use crp_diag::{DiagnosticSink, Discard};
    use serde_json::{Value, json};

    use super::*;
    use crate::publication::manifest::{InvalidManifest, Publication};

    /// Records advisory revocation failures without writing a process stream.
    #[derive(Debug, Default)]
    struct Recording(Mutex<Vec<String>>);

    impl DiagnosticSink for Recording {
        fn write(&self, text: &str) -> io::Result<()> {
            self.0.lock().unwrap().push(text.to_owned());
            Ok(())
        }
    }

    #[test]
    fn credential_protocol_issues_only_after_a_valid_request_and_never_caches() {
        let publication = publication();
        for operation in [
            request(),
            json!({"v":1,"kind":"get","operation":"read",
                "registry":{"index-url":"sparse+https://index.crates.io/"}}),
        ] {
            let mut output = Vec::new();
            serve_with(
                &publication,
                &mut Cursor::new(operation.to_string()),
                &mut output,
                || Ok("lease-canary".to_owned()),
            )
            .unwrap();
            let messages: Vec<Value> = String::from_utf8(output)
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert_eq!(
                messages,
                [
                    json!({"v":[1]}),
                    json!({"Ok":{"kind":"get","token":"lease-canary","cache":"never","operation_independent":false}}),
                ]
            );
        }
        let mut output = Vec::new();
        serve_with(&publication, &mut Cursor::new("{}"), &mut output, || {
            panic!("invalid request cannot acquire authority")
        })
        .unwrap();
        let messages: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(messages.first().unwrap(), &json!({"v":[1]}));
        assert_eq!(
            messages.last().unwrap().pointer("/Err/kind").unwrap(),
            "other"
        );
        let mut output = Vec::new();
        let error = serve_with(
            &publication,
            &mut Cursor::new(request().to_string()),
            &mut output,
            || Err(CredentialStateError::new().into()),
        )
        .unwrap_err();
        assert!(error.find_source::<CredentialStateError>().is_some());
        assert_eq!(
            serde_json::from_slice::<Value>(&output).unwrap(),
            json!({"v":[1]})
        );
    }

    #[test]
    fn issuance_retains_a_lease_or_revokes_it_before_returning_the_primary_failure() {
        let sink = Arc::new(Recording::default());
        let output = PublicationOutput::new("1.0.0", false, Arc::<Recording>::clone(&sink));
        let saved = Cell::new(false);
        let token = acquire_lease(
            || Ok("lease-canary".to_owned()),
            |token| {
                assert_eq!(token, "lease-canary");
                saved.set(true);
                Ok(())
            },
            |_| panic!("a retained lease belongs to parent cleanup"),
            &output,
        )
        .unwrap();
        assert!(saved.get());
        assert_eq!(token, "lease-canary");
        for reject_revocation in [false, true] {
            let revoked = Cell::new(false);
            let error = acquire_lease(
                || Ok("lease-canary".to_owned()),
                |_| Err(CredentialStateError::new().into()),
                |token| {
                    assert_eq!(token, "lease-canary");
                    revoked.set(true);
                    if reject_revocation {
                        Err(ProviderConfigurationError::new("revocation-canary").into())
                    } else {
                        Ok(())
                    }
                },
                &output,
            )
            .unwrap_err();
            assert!(revoked.get());
            assert!(error.find_source::<CredentialStateError>().is_some());
        }
        assert_eq!(sink.0.lock().unwrap().len(), 1);
        let error = acquire_lease(
            || Err(CredentialStateError::new().into()),
            |_| panic!("failed exchange has no lease"),
            |_| panic!("failed exchange has no token"),
            &output,
        )
        .unwrap_err();
        assert!(error.find_source::<CredentialStateError>().is_some());
    }

    #[test]
    fn lease_cleanup_skips_context_and_continues_after_each_independent_failure() {
        let output = PublicationOutput::new("1.0.0", false, Arc::new(Discard));
        let mut reads = Vec::new();
        let mut revocations = Vec::new();
        let mut removals = Vec::new();
        let entries = [
            Ok(PathBuf::from("context.json")),
            Err(CredentialStateError::new().into()),
            Ok(PathBuf::from("unreadable")),
            Ok(PathBuf::from("rejected")),
            Ok(PathBuf::from("retained")),
        ];
        let error = revoke_leases(
            entries.into_iter(),
            |path| {
                reads.push(path.to_path_buf());
                if path == Path::new("unreadable") {
                    Err(ReadFileError::new(path).into())
                } else {
                    Ok(path.to_str().unwrap().to_owned())
                }
            },
            |token| {
                revocations.push(token.to_owned());
                if token == "rejected" {
                    Err(ProviderConfigurationError::new("revocation").into())
                } else {
                    Ok(())
                }
            },
            |path| {
                removals.push(path.to_path_buf());
                if path == Path::new("retained") {
                    Err(WriteFileError::new(path).into())
                } else {
                    Ok(())
                }
            },
            &output,
        )
        .unwrap_err();
        assert_eq!(
            reads,
            ["unreadable", "rejected", "retained"].map(PathBuf::from)
        );
        assert_eq!(removals, reads);
        assert_eq!(revocations, ["rejected", "retained"]);
        assert!(error.find_source::<CredentialStateError>().is_some());
        let cleanup = error.find_source::<CredentialCleanupFailed>().unwrap();
        assert!(cleanup.cleanup.find_source::<WriteFileError>().is_some());
        revoke_leases(
            [Ok(PathBuf::from("token"))].into_iter(),
            |_| Ok("lease".into()),
            |token| {
                assert_eq!(token, "lease");
                Ok(())
            },
            |_| Ok(()),
            &output,
        )
        .unwrap();
    }

    #[test]
    fn finalization_closes_state_even_when_revocation_fails() {
        for revoked in [false, true] {
            for closed in [false, true] {
                let attempted = Cell::new(false);
                let result = finish_cleanup(
                    if revoked {
                        Ok(())
                    } else {
                        Err(CredentialStateError::new().into())
                    },
                    || {
                        attempted.set(true);
                        if closed {
                            Ok(())
                        } else {
                            Err(ProviderConfigurationError::new("close").into())
                        }
                    },
                );
                assert!(attempted.get());
                assert_eq!(result.is_ok(), revoked && closed);
                if let Err(error) = result {
                    if !revoked {
                        assert!(error.find_source::<CredentialStateError>().is_some());
                    }
                    if !closed {
                        let cleanup = error
                            .find_source::<CredentialCleanupFailed>()
                            .map_or(&error, |combined| &combined.cleanup);
                        assert!(
                            cleanup
                                .find_source::<ProviderConfigurationError>()
                                .is_some()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn cleanup_accumulation_retains_independent_failure_causes() {
        let output = PublicationOutput::new("1.0.0", false, Arc::new(Discard));
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
    fn registry_token_spelling_follows_the_platform_environment() {
        assert!(registry_token(OsStr::new("CARGO_REGISTRIES_PRIVATE_TOKEN")));
        assert_eq!(
            registry_token(OsStr::new("Cargo_Registries_Private_Token")),
            cfg!(windows)
        );
        for name in [
            "",
            "CARGO_REGISTRIES_PRIVATE_INDEX",
            "PREFIX_CARGO_REGISTRIES_PRIVATE_TOKEN",
            "CARGO_REGISTRIES_PRIVATE_TOKEN_SUFFIX",
            "CARGO_HOME",
            "RUSTUP_HOME",
            "RUSTFLAGS",
        ] {
            assert!(!registry_token(OsStr::new(name)));
        }
    }

    #[test]
    #[cfg_attr(
        all(miri, windows),
        ignore = "Command environment keys call Windows CompareStringOrdinal, unsupported by Miri"
    )]
    fn cargo_receives_no_publication_credentials() {
        let mut command = Command::new("cargo");
        strip_credentials(
            &mut command,
            [
                "CARGO_REGISTRIES_PRIVATE_TOKEN",
                "CARGO_REGISTRIES_PRIVATE_INDEX",
                "UNRELATED",
                "RUSTFLAGS",
                "CARGO_ENCODED_RUSTFLAGS",
                "CARGO_HOME",
                "RUSTUP_HOME",
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
        for name in [
            "UNRELATED",
            "CARGO_REGISTRIES_PRIVATE_INDEX",
            "RUSTFLAGS",
            "CARGO_ENCODED_RUSTFLAGS",
            "CARGO_HOME",
            "RUSTUP_HOME",
        ] {
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
