//! Registry availability reconciliation and Cargo-owned workspace uploads.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;
use std::{env, io, thread};

use crp_diag::Verbose;
use crp_workspace::artifact_path::write_new;
use crp_workspace::command::run_capture;
use ohno::AppError;
use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use semver::Version;
use serde::{Deserialize, Serialize};
use tempfile::Builder;

use crate::publication::candidate::{Repository, package_identifier};
use crate::publication::config::Configuration;
use crate::publication::context::WorkflowRun;
use crate::publication::credentials::CredentialSession;
use crate::publication::identity::{ActionsIdentity, TrustedPublisher};
use crate::publication::manifest::{InvalidManifest, PublicationManifest};
use crate::publication::packages::PublicationWorkspace;
use crate::publication::prepare::capture_requests;
use crate::{PublicationOutput, WriteFileError};

/// Observations for the exact version set in one immutable publication manifest.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryOutcome {
    pub schema_version: u32,
    pub publication_id: String,
    pub phase: String,
    pub dry_run: bool,
    pub complete: bool,
    pub packages: Vec<RegistryPackage>,
    pub errors: Vec<String>,
    pub notes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<WorkflowRun>,
}

impl RegistryOutcome {
    pub(crate) fn passed(&self) -> bool {
        self.errors.is_empty()
            && self.packages.iter().all(|package| {
                if self.dry_run {
                    matches!(
                        package.state,
                        RegistryState::AlreadyPresent | RegistryState::WouldPublish
                    )
                } else {
                    matches!(
                        package.state,
                        RegistryState::AlreadyPresent | RegistryState::Published
                    )
                }
            })
    }
}

/// The observed state of one requested version after this attempt.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RegistryPackage {
    pub name: String,
    pub version: String,
    pub state: RegistryState,
}

/// Distinguishes remote completeness from planned work and unavailable evidence.
#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegistryState {
    AlreadyPresent,
    Published,
    WouldPublish,
    Missing,
    Unknown,
}

/// Read-only crates.io version observations; authentication is only used by the upload provider.
#[derive(Debug)]
pub struct RegistryClient {
    client: Client,
    endpoint: String,
    output: PublicationOutput,
}

impl RegistryClient {
    pub fn new(output: PublicationOutput) -> Result<Self, AppError> {
        Self::with_endpoint("https://index.crates.io", output)
    }

    // Local HTTP services exercise the real adapter; the CLI's registry remains fixed.
    pub fn with_endpoint(endpoint: &str, output: PublicationOutput) -> Result<Self, AppError> {
        let client = Client::builder()
            .timeout(REGISTRY_QUERY_TIMEOUT)
            .user_agent(output.user_agent())
            .build()
            .map_err(RegistryQueryError::caused_by)?;
        Ok(Self {
            client,
            endpoint: endpoint.to_owned(),
            output,
        })
    }

    /// A yanked version still occupies its identity; Cargo decides dependency usability.
    pub fn contains(&self, name: &str, version: &str) -> Result<bool, AppError> {
        self.contains_with_wait(name, version, thread::sleep)
    }

    /// Uses the same retry decisions with a caller-owned delay boundary.
    pub fn contains_with_wait(
        &self,
        name: &str,
        version: &str,
        wait: impl FnMut(Duration),
    ) -> Result<bool, AppError> {
        self.versions(name, wait, false, |found, entry| {
            Ok(found || entry.version == version)
        })
    }

    pub fn exists(&self, name: &str) -> Result<bool, AppError> {
        self.versions(name, thread::sleep, false, |_, _| Ok(true))
    }

    /// Selects a fixed API-comparison version, preferring the highest non-yanked stable release.
    pub fn latest(&self, name: &str) -> Result<Option<Version>, AppError> {
        self.versions(name, thread::sleep, None, latest_observation)
    }

    /// Distinguishes a first publication from history with no usable comparison release.
    pub fn comparison_baseline(&self, name: &str) -> Result<Option<Version>, AppError> {
        let observation =
            self.versions(name, thread::sleep, (false, None), baseline_observation)?;
        comparison_baseline(name, observation)
    }

    fn versions<T>(
        &self,
        name: &str,
        wait: impl FnMut(Duration),
        initial: T,
        fold: impl FnMut(T, &RegistryVersion) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let url = format!("{}/{}", self.endpoint, index_path(name)?);
        let response = query_with_retry(
            || {
                self.client
                    .get(&url)
                    .send()
                    .map_err(RegistryQueryError::caused_by)
                    .map_err(Into::into)
            },
            Response::status,
            wait,
            &self.output,
        )?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(initial);
        }
        let response = response
            .error_for_status()
            .map_err(RegistryQueryError::caused_by)?;
        fold_index(BufReader::new(response), name, initial, fold)
    }
}

/// Acquires credentials and executes Cargo without changing registry or release policy.
///
/// Native boundary tests supply process outcomes and a controlled delay while retaining real
/// source verification and HTTP observations. The CLI always uses the native implementation.
pub trait RegistryRuntime {
    fn credentials(
        &self,
        publication: &PublicationManifest,
        manifest: &Path,
        target: &Path,
    ) -> Result<CredentialSession, AppError>;

    fn upload(&self, command: &mut Command) -> io::Result<Output>;

    fn pause(&self, delay: Duration);
}

/// Uses the authorized job identity, native Cargo process and infrastructure retry clock.
struct NativeRuntime<'a> {
    output: &'a PublicationOutput,
}

impl RegistryRuntime for NativeRuntime<'_> {
    fn credentials(
        &self,
        publication: &PublicationManifest,
        manifest: &Path,
        target: &Path,
    ) -> Result<CredentialSession, AppError> {
        CredentialSession::new(
            ActionsIdentity::from_environment()?,
            publication.clone(),
            manifest.to_path_buf(),
            target.to_path_buf(),
            TrustedPublisher::new(self.output.clone())?,
        )
    }

    fn upload(&self, command: &mut Command) -> io::Result<Output> {
        command.output()
    }

    fn pause(&self, delay: Duration) {
        thread::sleep(delay);
    }
}

fn fold_index<T>(
    mut input: impl BufRead,
    name: &str,
    mut result: T,
    mut fold: impl FnMut(T, &RegistryVersion) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let mut line = String::new();
    loop {
        line.clear();
        let read = input
            .read_line(&mut line)
            .map_err(RegistryQueryError::caused_by)?;
        if read == 0 {
            return Ok(result);
        }
        debug_assert!(
            !line.is_empty(),
            "a successful read must supply input before another iteration"
        );
        if line.trim().is_empty() {
            continue;
        }
        let entry: RegistryVersion =
            serde_json::from_str(&line).map_err(RegistryQueryError::caused_by)?;
        if entry.name != name {
            return Err(RegistryPackageMismatch::new(name.to_owned()).into());
        }
        // Validate the entire response even after an exact match; a malformed tail is not
        // evidence of a reliable registry observation.
        result = fold(result, &entry)?;
    }
}

fn latest_observation(
    selected: Option<Version>,
    entry: &RegistryVersion,
) -> Result<Option<Version>, AppError> {
    if entry.yanked {
        return Ok(selected);
    }
    let version = Version::parse(&entry.version)?;
    Ok(Some(match selected {
        Some(previous)
            if (previous.pre.is_empty() && !version.pre.is_empty())
                || (previous.pre.is_empty() == version.pre.is_empty() && previous >= version) =>
        {
            previous
        }
        _ => version,
    }))
}

fn baseline_observation(
    (_, selected): (bool, Option<Version>),
    entry: &RegistryVersion,
) -> Result<(bool, Option<Version>), AppError> {
    Ok((true, latest_observation(selected, entry)?))
}

fn comparison_baseline(
    name: &str,
    (exists, selected): (bool, Option<Version>),
) -> Result<Option<Version>, AppError> {
    if exists && selected.is_none() {
        return Err(ComparisonBaselineUnavailable::new(name.to_owned()).into());
    }
    Ok(selected)
}

/// Published history cannot be treated as a first release merely because all versions are yanked.
#[ohno::error]
#[display("package {package} has published history but no non-yanked comparison baseline")]
struct ComparisonBaselineUnavailable {
    package: String,
}

#[cfg(test)]
fn latest_version(entries: Vec<RegistryVersion>) -> Result<Option<Version>, AppError> {
    entries
        .into_iter()
        .try_fold(None, |selected, entry| latest_observation(selected, &entry))
}

fn query_with_retry<T>(
    mut query: impl FnMut() -> Result<T, AppError>,
    status: impl Fn(&T) -> StatusCode,
    mut wait: impl FnMut(Duration),
    diagnostics: &PublicationOutput,
) -> Result<T, AppError> {
    // This is a small operational retry allowance, not a crates.io service guarantee.
    // Space control-plane attempts to avoid immediate repeated throttling; keep the count
    // independent of upload propagation, whose purpose and query cost differ.
    const ATTEMPTS: usize = 3;
    const RETRY_DELAY: Duration = Duration::from_secs(5);
    for attempt in 1..=ATTEMPTS {
        let result = query();
        let retry = match &result {
            Ok(response) => {
                status(response) == StatusCode::TOO_MANY_REQUESTS
                    || status(response).is_server_error()
            }
            Err(_) => true,
        };
        if !retry || attempt == ATTEMPTS {
            return result;
        }
        if let Err(error) = &result {
            diagnostics.line(format_args!(
                "Registry query attempt {attempt} failed: {error}"
            ));
        } else {
            diagnostics.line(format_args!(
                "Registry query attempt {attempt} returned a transient status; retrying."
            ));
        }
        wait(RETRY_DELAY);
    }
    unreachable!("the last registry-query attempt returns")
}

/// Cargo-index evidence; REST visibility alone does not establish dependency availability.
#[derive(Deserialize)]
struct RegistryVersion {
    name: String,
    #[serde(rename = "vers")]
    version: String,
    #[serde(default)]
    yanked: bool,
}

fn index_path(name: &str) -> Result<String, AppError> {
    if !package_identifier(name) {
        return Err(InvalidManifest::new(
            "registry lookup requires a Cargo package name".to_owned(),
        )
        .into());
    }
    let name = name.to_ascii_lowercase();
    // This is Cargo's sparse registry sharding contract, not a release-policy choice.
    Ok(match name.len() {
        1 => format!("1/{name}"),
        2 => format!("2/{name}"),
        3 => format!(
            "3/{}/{name}",
            name.get(..1)
                .expect("a validated ASCII name has this prefix")
        ),
        _ => format!(
            "{}/{}/{name}",
            name.get(..2)
                .expect("a validated ASCII name has this prefix"),
            name.get(2..4)
                .expect("this branch has at least four ASCII bytes")
        ),
    })
}

// Allow slow metadata service responses without consuming the native build's budget.
// This engineering bound is per request; retry loops can spend multiple request allowances.
const REGISTRY_QUERY_TIMEOUT: Duration = Duration::from_secs(30);
/// Version of the registry receipt consumed by final reporting.
pub(crate) const OUTCOME_SCHEMA_VERSION: u32 = 1;

pub fn publish(
    publication_path: &Path,
    manifest_path: &Path,
    output: &Path,
    dry_run: bool,
    diagnostics: &PublicationOutput,
) -> Result<(bool, String), AppError> {
    if output
        .try_exists()
        .map_err(|error| WriteFileError::caused_by(output, error))?
    {
        return Err(InvalidManifest::new(
            "each publication attempt requires a new outcome destination".to_owned(),
        )
        .into());
    }
    let publication = PublicationManifest::read(publication_path)?;
    let client = RegistryClient::new(diagnostics.clone())?;
    let mut outcome = RegistryOutcome {
        schema_version: OUTCOME_SCHEMA_VERSION,
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
        github: WorkflowRun::capture()?,
    };
    let result = execute_with(
        &publication,
        manifest_path,
        &client,
        &mut outcome,
        diagnostics.notes(),
        &NativeRuntime {
            output: diagnostics,
        },
    );
    if let Err(error) = result {
        // Outcomes contain only a concise handoff; full typed diagnostics stay on stderr.
        diagnostics.line(format_args!("{error}"));
        outcome.errors.push(
            "Registry publication did not complete; inspect the command diagnostics.".to_owned(),
        );
    }
    outcome.complete = !dry_run && outcome.passed();
    write_outcome(output, &outcome)?;
    let passed = outcome.passed();
    Ok((
        passed,
        format!(
            "Registry {} for publication {}; outcome: {}.",
            if passed {
                "operation completed"
            } else {
                "operation failed"
            },
            outcome.publication_id,
            output.display()
        ),
    ))
}

/// Reconciles real source/index evidence with the invocation's credential and process boundary.
pub fn execute_with(
    publication: &PublicationManifest,
    manifest_path: &Path,
    client: &RegistryClient,
    outcome: &mut RegistryOutcome,
    verbose: Verbose<'_>,
    runtime: &impl RegistryRuntime,
) -> Result<(), AppError> {
    let repository = verify_source(publication, manifest_path)?;
    let mut missing = Vec::new();
    for package in &mut outcome.packages {
        if client.contains_with_wait(&package.name, &package.version, |delay| {
            runtime.pause(delay);
        })? {
            package.state = RegistryState::AlreadyPresent;
            verbose.note(|| {
                format!(
                    "{}@{} already occupies its crates.io identity, so no upload is requested.",
                    package.name, package.version
                )
            });
        } else {
            package.state = if outcome.dry_run {
                RegistryState::WouldPublish
            } else {
                RegistryState::Missing
            };
            missing.push(package.name.clone());
            verbose.note(|| format!(
                "{}@{} is absent from crates.io; this exact manifest request needs publication.",
                package.name, package.version
            ));
        }
    }
    verbose.note(|| format!(
        "Registry presence was checked for every exact manifest request; {} versions need upload. \
         Existing versions are retained independently of tags or version-assessment status.",
        missing.len()
    ));
    if missing.is_empty() || outcome.dry_run {
        return Ok(());
    }
    let workspace = PublicationWorkspace::load(manifest_path)?;
    require_workspace_publication(&run_capture("cargo", &["--version"], workspace.root())?)?;
    let source_manifest = workspace.root().join("Cargo.toml");
    // Cargo packaging output is not source. Keep even repositories without a target ignore
    // clean, while sharing one build directory across all requests in this invocation.
    let target = Builder::new()
        .prefix("cargo-release-plan-publish-")
        .tempdir()?;
    let session = runtime.credentials(publication, &source_manifest, target.path())?;
    let mut command = Command::new("cargo");
    command
        .args([
            "publish",
            "--registry",
            "crates-io",
            "--locked",
            "--manifest-path",
        ])
        .arg(&source_manifest)
        .current_dir(workspace.root())
        .env("CARGO_TARGET_DIR", target.path());
    for package in &missing {
        command.args(["--package", package]);
    }
    session.configure(&mut command, &env::current_exe()?)?;
    verbose.note(|| {
        "Cargo verifies and uploads the missing workspace set in dependency order; \
        the credential provider acquires a fresh temporary credential for each upload."
            .to_owned()
    });
    let upload = runtime.upload(&mut command);
    let cleanup = session.finish();
    let build_cleanup = target.close();
    let result = (|| {
        let upload = upload.map_err(RegistryUploadError::caused_by)?;
        client
            .output
            .best_effort_line(format_args!("{}", String::from_utf8_lossy(&upload.stderr)));
        client
            .output
            .best_effort_line(format_args!("{}", String::from_utf8_lossy(&upload.stdout)));
        observe_uploads(
            &mut outcome.packages,
            |name, version| client.contains_with_wait(name, version, |delay| runtime.pause(delay)),
            |delay| runtime.pause(delay),
        )?;
        repository.ensure_clean_head()?;
        if !upload.status.success()
            && outcome.packages.iter().all(|package| {
                matches!(
                    package.state,
                    RegistryState::AlreadyPresent | RegistryState::Published
                )
            })
        {
            outcome.notes.push(format!(
                "Cargo exited {}, but fresh registry observations confirm every requested version is available.",
                upload.status
            ));
        } else if !upload.status.success() {
            return Err(RegistryUploadFailed::new(upload.status.to_string()).into());
        }
        Ok(())
    })();
    let result = retain_cleanup(result, cleanup);
    retain_cleanup(
        result,
        build_cleanup.map_err(|error| RegistryUploadError::caused_by(error).into()),
    )
}

fn retain_cleanup(
    result: Result<(), AppError>,
    cleanup: Result<(), AppError>,
) -> Result<(), AppError> {
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => Err(RegistryCleanupFailed::caused_by(cleanup, error).into()),
    }
}

/// Keeps finalization failures alongside the original upload or observation error.
#[ohno::error]
#[display("registry finalization also failed: {cleanup}")]
struct RegistryCleanupFailed {
    cleanup: AppError,
}

fn observe_uploads(
    packages: &mut [RegistryPackage],
    mut contains: impl FnMut(&str, &str) -> Result<bool, AppError>,
    mut wait: impl FnMut(Duration),
) -> Result<(), AppError> {
    // Cargo already waits on its index view. This separate engineering allowance gives
    // the reader repeated observations spaced across propagation, shared by all packages.
    // Its cadence need not track transport retry tuning; each query has its own retry budget.
    const ATTEMPTS: usize = 6;
    const DELAY: Duration = Duration::from_secs(5);
    for attempt in 1..=ATTEMPTS {
        let mut missing = false;
        for package in packages
            .iter_mut()
            .filter(|package| package.state == RegistryState::Missing)
        {
            let available = match contains(&package.name, &package.version) {
                Ok(available) => available,
                Err(error) => {
                    package.state = RegistryState::Unknown;
                    return Err(error);
                }
            };
            if available {
                package.state = RegistryState::Published;
            } else {
                missing = true;
            }
        }
        if !missing || attempt == ATTEMPTS {
            return Ok(());
        }
        wait(DELAY);
    }
    unreachable!("the last index-observation attempt returns")
}

pub(crate) fn verify_source(
    publication: &PublicationManifest,
    manifest: &Path,
) -> Result<Repository, AppError> {
    let repository = Repository::discover(manifest, &publication.publication.source)?;
    repository.ensure_clean_head()?;
    let workspace = PublicationWorkspace::load(manifest)?;
    let workspace_manifest = repository.require_tracked(&workspace.root().join("Cargo.toml"))?;
    let expected = repository.require_tracked(
        &repository
            .root()
            .join(&publication.publication.workspace_manifest),
    )?;
    if workspace_manifest != expected {
        return Err(InvalidManifest::new(
            "selected workspace differs from publication intent".to_owned(),
        )
        .into());
    }
    let (_, config) = Configuration::load(
        workspace.root(),
        Some(&repository.root().join(&publication.publication.config_path)),
    )?;
    if config != publication.publication.configuration {
        return Err(InvalidManifest::new(
            "source configuration differs from publication intent".to_owned(),
        )
        .into());
    }
    let requests = capture_requests(&repository, workspace.requests(&config)?)?;
    if requests != publication.publication.packages {
        return Err(InvalidManifest::new(
            "source packages differ from publication intent".to_owned(),
        )
        .into());
    }
    Ok(repository)
}

pub(crate) fn write_outcome(path: &Path, outcome: &impl Serialize) -> Result<(), AppError> {
    write_new(path, |file| {
        serde_json::to_writer_pretty(file, outcome)
            .map_err(|error| WriteFileError::caused_by(path, error).into())
    })
}

fn require_workspace_publication(reported: &str) -> Result<(), AppError> {
    // Native boundary tests cover workspace upload ordering and credential timing at this floor.
    const MINIMUM_CARGO: Version = Version::new(1, 95, 0);
    let version = reported
        .strip_prefix("cargo ")
        .and_then(|value| value.split_whitespace().next())
        .ok_or_else(|| UnsupportedCargo::new(reported.trim().to_owned()))?;
    let version = Version::parse(version)
        .map_err(|error| UnsupportedCargo::caused_by(reported.trim().to_owned(), error))?;
    if version < MINIMUM_CARGO {
        return Err(UnsupportedCargo::new(reported.trim().to_owned()).into());
    }
    Ok(())
}

#[ohno::error]
#[display("workspace publication requires Cargo 1.95 or later; reported {reported}")]
struct UnsupportedCargo {
    reported: String,
}

#[ohno::error]
#[display("cannot determine exact crates.io version availability")]
struct RegistryQueryError;

#[ohno::error]
#[display("registry index response does not identify requested package {package}")]
struct RegistryPackageMismatch {
    package: String,
}

#[ohno::error]
#[display("cannot execute Cargo registry publication")]
struct RegistryUploadError;

#[ohno::error]
#[display("Cargo publication failed ({status}); completed uploads remain published")]
struct RegistryUploadFailed {
    status: String,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn finalization_keeps_primary_and_cleanup_failures() {
        let error = retain_cleanup(
            Err(RegistryUploadFailed::new("rejected".to_owned()).into()),
            Err(RegistryQueryError::new().into()),
        )
        .unwrap_err();
        assert!(error.find_source::<RegistryUploadFailed>().is_some());
        let finalization = error.find_source::<RegistryCleanupFailed>().unwrap();
        assert!(
            finalization
                .cleanup
                .find_source::<RegistryQueryError>()
                .is_some()
        );
    }

    #[test]
    fn streamed_index_checks_every_record_without_retaining_history() {
        let input = b"{\"name\":\"tool\",\"vers\":\"1.0.0\"}\n{\"name\":\"tool\",\"vers\":\"2.0.0\",\"yanked\":true}\n";
        assert!(
            fold_index(&input[..], "tool", false, |found, entry| Ok(
                found || entry.version == "2.0.0"
            ))
            .unwrap()
        );
        assert_eq!(
            fold_index(&input[..], "tool", None, latest_observation).unwrap(),
            Some(Version::new(1, 0, 0))
        );
        for tail in ["not-json", "{\"name\":\"other\",\"vers\":\"1.0.0\"}"] {
            let input = format!("{{\"name\":\"tool\",\"vers\":\"1.0.0\"}}\n{tail}");
            fold_index(input.as_bytes(), "tool", false, |_, _| Ok(true)).unwrap_err();
        }
    }

    #[test]
    fn comparison_baseline_distinguishes_absence_from_unusable_history() {
        let absent = fold_index(&b""[..], "tool", (false, None), baseline_observation).unwrap();
        assert!(comparison_baseline("tool", absent).unwrap().is_none());
        let yanked = b"{\"name\":\"tool\",\"vers\":\"1.0.0\",\"yanked\":true}\n";
        let yanked = fold_index(&yanked[..], "tool", (false, None), baseline_observation).unwrap();
        let error = comparison_baseline("tool", yanked).unwrap_err();
        assert!(
            error
                .find_source::<ComparisonBaselineUnavailable>()
                .is_some()
        );
        let published = b"{\"name\":\"tool\",\"vers\":\"3.0.0-beta.1\"}\n{\"name\":\"tool\",\"vers\":\"1.0.0\"}\n";
        let published =
            fold_index(&published[..], "tool", (false, None), baseline_observation).unwrap();
        assert_eq!(
            comparison_baseline("tool", published).unwrap(),
            Some(Version::new(1, 0, 0))
        );
    }

    #[test]
    fn comparison_baseline_prefers_non_yanked_stable_versions() {
        for (versions, expected) in [
            (json!([]), None),
            (json!([{"name":"tool","vers":"2.0.0","yanked":true}]), None),
            (
                json!([
                    {"name":"tool","vers":"3.0.0-beta.2"},
                    {"name":"tool","vers":"3.0.0-beta.1"}
                ]),
                Some("3.0.0-beta.2"),
            ),
            (
                json!([
                    {"name":"tool","vers":"3.0.0-beta.2"},
                    {"name":"tool","vers":"2.0.0"},
                    {"name":"tool","vers":"1.0.0"},
                    {"name":"tool","vers":"4.0.0","yanked":true}
                ]),
                Some("2.0.0"),
            ),
        ] {
            assert_eq!(
                latest_version(serde_json::from_value(versions).unwrap()).unwrap(),
                expected.map(|version| Version::parse(version).unwrap())
            );
        }
        latest_version(
            serde_json::from_value(json!([{"name":"tool","vers":"not-a-version"}])).unwrap(),
        )
        .unwrap_err();
    }

    #[test]
    fn upload_observation_waits_for_missing_versions_without_requerying_completed_work() {
        let mut packages = vec![
            RegistryPackage {
                name: "existing".to_owned(),
                version: "1.0.0".to_owned(),
                state: RegistryState::AlreadyPresent,
            },
            RegistryPackage {
                name: "new".to_owned(),
                version: "1.0.0".to_owned(),
                state: RegistryState::Missing,
            },
        ];
        let mut queries = 0;
        let mut waits = 0;
        observe_uploads(
            &mut packages,
            |name, _| {
                assert_eq!(name, "new");
                queries += 1;
                Ok(queries == 3)
            },
            |_| waits += 1,
        )
        .unwrap();
        assert_eq!(queries, 3);
        assert_eq!(waits, 2);
        assert_eq!(packages.last().unwrap().state, RegistryState::Published);
        packages.last_mut().unwrap().state = RegistryState::Missing;
        let mut attempts = 0;
        observe_uploads(
            &mut packages,
            |_, _| {
                attempts += 1;
                Ok(false)
            },
            |_| {},
        )
        .unwrap();
        assert_eq!(attempts, 6);
        assert_eq!(packages.last().unwrap().state, RegistryState::Missing);
        observe_uploads(
            &mut packages,
            |_, _| Err(RegistryQueryError::new().into()),
            |_| {},
        )
        .unwrap_err();
        assert_eq!(packages.last().unwrap().state, RegistryState::Unknown);
    }

    #[test]
    fn registry_metadata_retries_only_transient_failures_with_a_fixed_budget() {
        let mut responses = [
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::OK,
        ]
        .into_iter();
        let mut waits = 0;
        let result = query_with_retry(
            || Ok(responses.next().unwrap()),
            |status| *status,
            |_| waits += 1,
            &PublicationOutput::new("1.2.3", false, std::sync::Arc::new(crp_diag::Discard)),
        )
        .unwrap();
        assert_eq!(result, StatusCode::OK);
        assert_eq!(waits, 2);
        query_with_retry(
            || Ok(StatusCode::FORBIDDEN),
            |status| *status,
            |_| panic!("deterministic status must not retry"),
            &PublicationOutput::new("1.2.3", false, std::sync::Arc::new(crp_diag::Discard)),
        )
        .unwrap();
        let mut attempts = 0;
        let result = query_with_retry(
            || {
                attempts += 1;
                Ok(StatusCode::SERVICE_UNAVAILABLE)
            },
            |status| *status,
            |_| {},
            &PublicationOutput::new("1.2.3", false, std::sync::Arc::new(crp_diag::Discard)),
        )
        .unwrap();
        assert_eq!(result, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(attempts, 3);
    }

    #[test]
    fn registry_index_paths_follow_cargos_package_sharding() {
        for (name, expected) in [
            ("a", "1/a"),
            ("ab", "2/ab"),
            ("abc", "3/a/abc"),
            ("ab_cd", "ab/_c/ab_cd"),
        ] {
            assert_eq!(index_path(name).unwrap(), expected);
        }
        index_path("../outside").unwrap_err();
    }

    #[test]
    fn requires_the_proven_workspace_publication_capability() {
        for version in ["cargo 1.95.0 (fixture)", "cargo 1.98.1 (fixture)"] {
            require_workspace_publication(version).unwrap();
        }
        for version in ["cargo 1.94.0", "cargo unknown", "rustc 1.98.1", ""] {
            require_workspace_publication(version).unwrap_err();
        }
    }

    #[test]
    fn only_complete_observations_or_explicit_dry_run_plans_pass() {
        for dry_run in [false, true] {
            for (state, execute_passes, dry_passes) in [
                (RegistryState::AlreadyPresent, true, true),
                (RegistryState::Published, true, false),
                (RegistryState::WouldPublish, false, true),
                (RegistryState::Missing, false, false),
                (RegistryState::Unknown, false, false),
            ] {
                let mut outcome = RegistryOutcome {
                    schema_version: OUTCOME_SCHEMA_VERSION,
                    publication_id: "intent".to_owned(),
                    phase: "registry".to_owned(),
                    dry_run,
                    complete: false,
                    packages: vec![RegistryPackage {
                        name: "library".to_owned(),
                        version: "1.0.0".to_owned(),
                        state,
                    }],
                    errors: Vec::new(),
                    notes: Vec::new(),
                    github: None,
                };
                assert_eq!(
                    outcome.passed(),
                    if dry_run { dry_passes } else { execute_passes }
                );
                outcome.errors.push("failed cleanup".to_owned());
                assert!(!outcome.passed());
            }
        }
    }
}
