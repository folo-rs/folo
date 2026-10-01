//! Registry availability reconciliation and Cargo-owned workspace uploads.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;
use std::{env, io, thread};

use crp_diag::Verbose;
use crp_workspace::command::run_capture;
use ohno::AppError;
use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use semver::Version;
use serde::{Deserialize, Serialize};
use tempfile::Builder;

use crate::PublicationOutput;
use crate::publication::artifact::{require_new, write_outcome};
use crate::publication::candidate::package_identifier;
use crate::publication::context::WorkflowRun;
use crate::publication::credentials::CredentialSession;
use crate::publication::identity::{ActionsIdentity, TrustedPublisher};
use crate::publication::manifest::{InvalidManifest, PublicationManifest};
use crate::publication::source::verify_source;

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
    // Real HTTP/default delay wiring is covered by publication_registry's local index tests.
    #[cfg_attr(test, mutants::skip)]
    pub fn contains(&self, name: &str, version: &str) -> Result<bool, AppError> {
        self.contains_with_wait(name, version, thread::sleep)
    }

    /// Uses the same retry decisions with a caller-owned delay boundary.
    // The adapter acquires HTTP data; contains_observation tests exact-version accumulation.
    #[cfg_attr(test, mutants::skip)]
    pub fn contains_with_wait(
        &self,
        name: &str,
        version: &str,
        wait: impl FnMut(Duration),
    ) -> Result<bool, AppError> {
        self.versions(name, wait, false, |found, entry| {
            Ok(contains_observation(found, entry, version))
        })
    }

    // HTTP existence/absence forwarding is covered by the local index integration.
    #[cfg_attr(test, mutants::skip)]
    pub fn exists(&self, name: &str) -> Result<bool, AppError> {
        self.versions(name, thread::sleep, false, |_, _| Ok(true))
    }

    /// Selects a fixed API-comparison version, preferring the highest non-yanked stable release.
    // The local index integration covers acquisition; latest_observation owns version preference.
    #[cfg_attr(test, mutants::skip)]
    pub fn latest(&self, name: &str) -> Result<Option<Version>, AppError> {
        self.versions(name, thread::sleep, None, latest_observation)
    }

    /// Distinguishes a first publication from history with no usable comparison release.
    // Local HTTP integration covers the adapter; baseline_observation/comparison_baseline
    // independently test absence versus unusable history.
    #[cfg_attr(test, mutants::skip)]
    pub fn comparison_baseline(&self, name: &str) -> Result<Option<Version>, AppError> {
        let observation =
            self.versions(name, thread::sleep, (false, None), baseline_observation)?;
        comparison_baseline(name, observation)
    }

    // HTTP request/response I/O is covered by publication_registry. Retry, status interpretation
    // and streamed decoding remain independently unit-testable.
    #[cfg_attr(test, mutants::skip)]
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
        index_response(response.status(), initial, |initial| {
            let response = response
                .error_for_status()
                .map_err(RegistryQueryError::caused_by)?;
            fold_index(BufReader::new(response), name, initial, fold)
        })
    }
}

fn index_response<T>(
    status: StatusCode,
    initial: T,
    read: impl FnOnce(T) -> Result<T, AppError>,
) -> Result<T, AppError> {
    if status == StatusCode::NOT_FOUND {
        Ok(initial)
    } else {
        read(initial)
    }
}

fn contains_observation(found: bool, entry: &RegistryVersion, version: &str) -> bool {
    found || entry.version == version
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
    ) -> Result<CredentialSession, AppError> {
        CredentialSession::new(
            ActionsIdentity::from_environment()?,
            publication.clone(),
            manifest.to_path_buf(),
            TrustedPublisher::new(self.output.clone())?,
        )
    }

    fn upload(&self, command: &mut Command) -> io::Result<Output> {
        command.output()
    }

    // Timing-only adapter: waiting in tests cannot establish correctness without real time.
    // Retry/propagation tests assert the requested delays using a simulated wait callback.
    #[cfg_attr(test, mutants::skip)]
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
#[cfg_attr(coverage_nightly, coverage(off))]
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
    // Space registry-query attempts apart to avoid immediate repeated throttling; keep the count
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

// Files, process identity and production service selection are native wiring, covered by
// executable publication tests. finish_outcome tests the persisted verdict separately.
#[cfg_attr(test, mutants::skip)]
pub fn publish(
    publication_path: &Path,
    manifest_path: &Path,
    output: &Path,
    dry_run: bool,
    diagnostics: &PublicationOutput,
) -> Result<(bool, String), AppError> {
    require_new(output)?;
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
    finish_outcome(&mut outcome, result, diagnostics);
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

fn finish_outcome(
    outcome: &mut RegistryOutcome,
    result: Result<(), AppError>,
    diagnostics: &PublicationOutput,
) {
    if let Err(error) = result {
        // Outcomes contain only a concise handoff; full typed diagnostics stay on stderr.
        diagnostics.line(format_args!("{error}"));
        outcome.errors.push(
            "Registry publication did not complete; inspect the command diagnostics.".to_owned(),
        );
    }
    outcome.complete = !outcome.dry_run && outcome.passed();
}

/// Reconciles real source/index evidence with the invocation's credential and process boundary.
// publication_registry covers the real Git/Cargo/credential/HTTP composition. Selection,
// confirmation, retry budgets, reporting and cleanup decisions are exercised in process.
#[cfg_attr(test, mutants::skip)]
pub fn execute_with(
    publication: &PublicationManifest,
    manifest_path: &Path,
    client: &RegistryClient,
    outcome: &mut RegistryOutcome,
    verbose: Verbose<'_>,
    runtime: &impl RegistryRuntime,
) -> Result<(), AppError> {
    let source = verify_source(publication, manifest_path)?;
    let missing = select_uploads(
        outcome,
        |name, version| client.contains_with_wait(name, version, |delay| runtime.pause(delay)),
        verbose,
    )?;
    if missing.is_empty() {
        return Ok(());
    }
    let source_manifest = source.manifest;
    let workspace = source_manifest
        .parent()
        .expect("verified workspace manifest has a parent");
    require_workspace_publication(&run_capture("cargo", &["--version"], workspace)?)?;
    // Cargo packaging output is not source. Keep even repositories without a target ignore
    // clean, while sharing one build directory across all requests in this invocation.
    let target = Builder::new()
        .prefix("cargo-release-plan-publish-")
        .tempdir()
        .map_err(RegistryBuildStateCreationFailed::caused_by)?;
    let target_path = target.path().to_path_buf();
    let session = runtime.credentials(publication, &source_manifest)?;
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
        .current_dir(workspace)
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
        let confirmation = observe_uploads(
            &mut outcome.packages,
            |name, version| client.contains_with_wait(name, version, |delay| runtime.pause(delay)),
            |delay| runtime.pause(delay),
        );
        let publication_complete = outcome.packages.iter().all(|package| {
            matches!(
                package.state,
                RegistryState::AlreadyPresent | RegistryState::Published
            )
        });
        confirm_upload(
            upload.status.success(),
            &upload.status.to_string(),
            confirmation,
            publication_complete,
        )?;
        source.repository.ensure_clean_head()?;
        record_upload_status(outcome, upload.status.success(), &upload.status.to_string());
        Ok(())
    })();
    let result = retain_cleanup(result, cleanup);
    retain_cleanup(
        result,
        build_cleanup
            .map_err(|error| RegistryBuildStateCleanupFailed::caused_by(target_path, error).into()),
    )
}

fn select_uploads(
    outcome: &mut RegistryOutcome,
    mut contains: impl FnMut(&str, &str) -> Result<bool, AppError>,
    verbose: Verbose<'_>,
) -> Result<Vec<String>, AppError> {
    let mut missing = Vec::new();
    for package in &mut outcome.packages {
        if contains(&package.name, &package.version)? {
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
    verbose.note(|| {
        "Registry presence was checked for every exact manifest request. \
        Absent versions require upload; existing versions are retained independently of \
        tags or version-assessment status."
            .to_owned()
    });
    if outcome.dry_run {
        missing.clear();
    }
    Ok(missing)
}

fn record_upload_status(outcome: &mut RegistryOutcome, success: bool, status: &str) {
    if !success {
        outcome.notes.push(format!(
            "Cargo exited {status}, but fresh registry observations confirm every requested version is available."
        ));
    }
}

fn confirm_upload(
    success: bool,
    status: &str,
    confirmation: Result<(), AppError>,
    complete: bool,
) -> Result<(), AppError> {
    match confirmation {
        Ok(()) if success || complete => Ok(()),
        Ok(()) => Err(RegistryUploadFailed::new(status.to_owned()).into()),
        Err(error) if success => Err(error),
        Err(confirmation) => Err(RegistryConfirmationFailed::caused_by(
            confirmation,
            RegistryUploadFailed::new(status.to_owned()),
        )
        .into()),
    }
}

/// Confirmation cannot erase the failed upload whose remote result remains unknown.
#[ohno::error]
#[display("registry upload confirmation also failed: {confirmation}")]
struct RegistryConfirmationFailed {
    confirmation: AppError,
}

#[ohno::error]
#[display("cannot create temporary registry build state")]
struct RegistryBuildStateCreationFailed;

#[ohno::error]
#[display("cannot remove temporary registry build state {}", path.display())]
struct RegistryBuildStateCleanupFailed {
    path: PathBuf,
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
        let mut failure = None;
        for package in packages
            .iter_mut()
            .filter(|package| package.state == RegistryState::Missing)
        {
            let available = match contains(&package.name, &package.version) {
                Ok(available) => available,
                Err(error) => {
                    package.state = RegistryState::Unknown;
                    let error = RegistryPackageObservationFailed::caused_by(
                        package.name.clone(),
                        package.version.clone(),
                        error,
                    );
                    failure = Some(match failure {
                        None => AppError::from(error),
                        Some(previous) => {
                            RegistryObservationFailures::caused_by(error, previous).into()
                        }
                    });
                    continue;
                }
            };
            if available {
                package.state = RegistryState::Published;
            } else {
                missing = true;
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        if !missing || attempt == ATTEMPTS {
            return Ok(());
        }
        wait(DELAY);
    }
    unreachable!("the last index-observation attempt returns")
}

/// Independent package observations remain available even if another lookup fails.
#[ohno::error]
#[display("cannot confirm post-upload availability of {package}@{version}")]
struct RegistryPackageObservationFailed {
    package: String,
    version: String,
}

#[ohno::error]
#[display("another post-upload observation failed: {additional}")]
struct RegistryObservationFailures {
    additional: AppError,
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
    fn exact_version_observation_retains_matches_and_checks_later_records() {
        for (versions, expected) in [
            (vec![], false),
            (vec!["1.0.0"], true),
            (vec!["2.0.0"], false),
            (vec!["1.0.0", "2.0.0"], true),
            (vec!["2.0.0", "1.0.0"], true),
        ] {
            let input = versions
                .into_iter()
                .map(|version| json!({"name":"tool","vers":version,"yanked":true}).to_string())
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(
                fold_index(input.as_bytes(), "tool", false, |found, entry| {
                    Ok(contains_observation(found, entry, "1.0.0"))
                })
                .unwrap(),
                expected
            );
        }
        let input = b"{\"name\":\"tool\",\"vers\":\"1.0.0\"}\ninvalid-tail";
        assert!(
            fold_index(&input[..], "tool", false, |found, entry| {
                Ok(contains_observation(found, entry, "1.0.0"))
            })
            .unwrap_err()
            .find_source::<RegistryQueryError>()
            .is_some()
        );
    }

    #[test]
    fn only_not_found_bypasses_response_validation() {
        assert_eq!(
            index_response(StatusCode::NOT_FOUND, "absent", |_| {
                panic!("a not-found body is not an index");
            })
            .unwrap(),
            "absent"
        );
        assert_eq!(
            index_response(StatusCode::OK, "initial", |initial| {
                assert_eq!(initial, "initial");
                Ok("observed")
            })
            .unwrap(),
            "observed"
        );
        let error = index_response(StatusCode::FORBIDDEN, false, |_| {
            Err(RegistryQueryError::new().into())
        })
        .unwrap_err();
        assert!(error.find_source::<RegistryQueryError>().is_some());
    }

    fn pending_outcome(dry_run: bool) -> RegistryOutcome {
        RegistryOutcome {
            schema_version: OUTCOME_SCHEMA_VERSION,
            publication_id: "intent".to_owned(),
            phase: "registry".to_owned(),
            dry_run,
            complete: false,
            packages: ["present", "missing"]
                .map(|name| RegistryPackage {
                    name: name.to_owned(),
                    version: "1.0.0".to_owned(),
                    state: RegistryState::Unknown,
                })
                .into_iter()
                .collect(),
            errors: vec![],
            notes: vec![],
            github: None,
        }
    }

    #[test]
    fn upload_selection_observes_every_request_but_never_uploads_dry_run_or_present_work() {
        let output = PublicationOutput::new("1.0.0", false, std::sync::Arc::new(crp_diag::Discard));
        for dry_run in [false, true] {
            let mut outcome = pending_outcome(dry_run);
            let mut queries = Vec::new();
            let selected = select_uploads(
                &mut outcome,
                |name, version| {
                    queries.push((name.to_owned(), version.to_owned()));
                    Ok(name == "present")
                },
                output.notes(),
            )
            .unwrap();
            assert_eq!(
                queries,
                [
                    ("present".to_owned(), "1.0.0".to_owned()),
                    ("missing".to_owned(), "1.0.0".to_owned()),
                ]
            );
            assert_eq!(selected, if dry_run { vec![] } else { vec!["missing"] });
            assert_eq!(
                outcome.packages.first().unwrap().state,
                RegistryState::AlreadyPresent
            );
            assert_eq!(
                outcome.packages.last().unwrap().state,
                if dry_run {
                    RegistryState::WouldPublish
                } else {
                    RegistryState::Missing
                }
            );
            assert!(
                select_uploads(&mut outcome, |_, _| Ok(true), output.notes())
                    .unwrap()
                    .is_empty()
            );
            let error = select_uploads(
                &mut outcome,
                |_, _| Err(RegistryQueryError::new().into()),
                output.notes(),
            )
            .unwrap_err();
            assert!(error.find_source::<RegistryQueryError>().is_some());
        }
    }

    #[test]
    fn confirmed_upload_status_retains_only_unsuccessful_cargo_exits_as_notes() {
        let mut outcome = pending_outcome(false);
        record_upload_status(&mut outcome, true, "success");
        assert!(outcome.notes.is_empty());
        record_upload_status(&mut outcome, false, "exit-canary");
        assert_eq!(outcome.notes.len(), 1);
        assert!(outcome.notes.first().unwrap().contains("exit-canary"));
    }

    #[test]
    fn failed_post_upload_query_does_not_leave_later_results_unobserved() {
        let mut packages: Vec<_> = ["first", "second", "third", "fourth"]
            .into_iter()
            .map(|name| RegistryPackage {
                name: name.to_owned(),
                version: "1.0.0".to_owned(),
                state: RegistryState::Missing,
            })
            .collect();
        let mut queries = Vec::new();
        let error = observe_uploads(
            &mut packages,
            |name, _| {
                queries.push(name.to_owned());
                match name {
                    "first" | "fourth" => Err(RegistryQueryError::new().into()),
                    "second" => Ok(true),
                    "third" => Ok(false),
                    _ => panic!("unexpected package"),
                }
            },
            |_| panic!("a failed observation pass must not start another retry"),
        )
        .unwrap_err();
        assert_eq!(queries, ["first", "second", "third", "fourth"]);
        assert_eq!(
            packages
                .iter()
                .map(|package| &package.state)
                .collect::<Vec<_>>(),
            [
                &RegistryState::Unknown,
                &RegistryState::Published,
                &RegistryState::Missing,
                &RegistryState::Unknown
            ]
        );
        assert_eq!(
            error
                .find_source::<RegistryPackageObservationFailed>()
                .unwrap()
                .package,
            "first"
        );
        assert_eq!(
            error
                .find_source::<RegistryObservationFailures>()
                .unwrap()
                .additional
                .find_source::<RegistryPackageObservationFailed>()
                .unwrap()
                .package,
            "fourth"
        );
    }

    #[test]
    fn failed_upload_keeps_its_status_when_confirmation_also_fails() {
        let error = confirm_upload(
            false,
            "failure",
            Err(RegistryQueryError::new().into()),
            false,
        )
        .unwrap_err();
        assert!(error.find_source::<RegistryUploadFailed>().is_some());
        assert!(
            error
                .find_source::<RegistryConfirmationFailed>()
                .unwrap()
                .confirmation
                .find_source::<RegistryQueryError>()
                .is_some()
        );
        confirm_upload(false, "failure", Ok(()), true).unwrap();
        assert!(
            confirm_upload(false, "failure", Ok(()), false)
                .unwrap_err()
                .find_source::<RegistryUploadFailed>()
                .is_some()
        );
        assert!(
            confirm_upload(
                true,
                "success",
                Err(RegistryQueryError::new().into()),
                false
            )
            .unwrap_err()
            .find_source::<RegistryQueryError>()
            .is_some()
        );
    }

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
                    {"name":"tool","vers":"1.0.0"},
                    {"name":"tool","vers":"2.0.0"},
                    {"name":"tool","vers":"3.0.0-beta.1"}
                ]),
                Some("2.0.0"),
            ),
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
                let diagnostics =
                    PublicationOutput::new("1.0.0", false, std::sync::Arc::new(crp_diag::Discard));
                finish_outcome(&mut outcome, Ok(()), &diagnostics);
                assert_eq!(outcome.complete, !dry_run && execute_passes);
                finish_outcome(
                    &mut outcome,
                    Err(RegistryQueryError::new().into()),
                    &diagnostics,
                );
                assert!(!outcome.complete);
                assert_eq!(outcome.errors.len(), 1);
                outcome.errors.push("failed cleanup".to_owned());
                assert!(!outcome.passed());
            }
        }
    }
}
