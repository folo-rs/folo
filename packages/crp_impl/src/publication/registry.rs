//! Registry availability reconciliation and Cargo-owned workspace uploads.

use std::path::Path;
use std::process::Command;
use std::time::Duration;
use std::{env, fs, thread};

use ohno::AppError;
use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};
use semver::Version;
use serde::{Deserialize, Serialize};
use tempfile::{Builder, NamedTempFile};

use crate::WriteFileError;
use crate::command::run_capture;
use crate::publication::candidate::{Repository, package_identifier};
use crate::publication::config::Configuration;
use crate::publication::context::WorkflowRun;
use crate::publication::credentials::CredentialSession;
use crate::publication::identity::{ActionsIdentity, TrustedPublisher};
use crate::publication::manifest::{InvalidManifest, PublicationManifest};
use crate::publication::packages::PublicationWorkspace;
use crate::publication::prepare::capture_requests;
use crate::verbose::Verbose;

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
    fn passed(&self) -> bool {
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
}

impl RegistryClient {
    pub fn new() -> Result<Self, AppError> {
        Self::with_endpoint("https://index.crates.io")
    }

    // Local HTTP services exercise the real adapter; the CLI's registry remains fixed.
    pub fn with_endpoint(endpoint: &str) -> Result<Self, AppError> {
        let client = Client::builder()
            .timeout(REGISTRY_QUERY_TIMEOUT)
            .user_agent(concat!("cargo-release-plan/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(RegistryQueryError::caused_by)?;
        Ok(Self {
            client,
            endpoint: endpoint.to_owned(),
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
        Ok(self
            .versions(name, wait)?
            .iter()
            .any(|entry| entry.version == version))
    }

    pub(crate) fn exists(&self, name: &str) -> Result<bool, AppError> {
        Ok(!self.versions(name, thread::sleep)?.is_empty())
    }

    /// Selects a fixed API-comparison version, preferring the highest non-yanked stable release.
    pub(crate) fn latest(&self, name: &str) -> Result<Option<Version>, AppError> {
        let entries = self.versions(name, thread::sleep)?;
        let mut versions = Vec::new();
        for entry in entries {
            if !entry.yanked {
                versions.push(Version::parse(&entry.version)?);
            }
        }
        let stable = versions
            .iter()
            .filter(|version| version.pre.is_empty())
            .max()
            .cloned();
        Ok(stable.or_else(|| versions.into_iter().max()))
    }

    fn versions(
        &self,
        name: &str,
        wait: impl FnMut(Duration),
    ) -> Result<Vec<RegistryVersion>, AppError> {
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
        )?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        let response = response
            .error_for_status()
            .map_err(RegistryQueryError::caused_by)?;
        let contents = response.text().map_err(RegistryQueryError::caused_by)?;
        let mut entries = Vec::new();
        for line in contents.lines().filter(|line| !line.is_empty()) {
            let entry: RegistryVersion =
                serde_json::from_str(line).map_err(RegistryQueryError::caused_by)?;
            if entry.name != name {
                return Err(RegistryVersionMismatch::new(
                    name.to_owned(),
                    "index entry".to_owned(),
                )
                .into());
            }
            entries.push(entry);
        }
        Ok(entries)
    }
}

fn query_with_retry<T>(
    mut query: impl FnMut() -> Result<T, AppError>,
    status: impl Fn(&T) -> StatusCode,
    mut wait: impl FnMut(Duration),
) -> Result<T, AppError> {
    // Metadata endpoints occasionally return rate-limit or transient server failures.
    // Bound retries independently of a long Cargo upload and do not retry deterministic 4xx.
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
            eprintln!("Registry query attempt {attempt} failed: {error}");
        } else {
            eprintln!("Registry query attempt {attempt} returned a transient status; retrying.");
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

// Registry queries are short metadata operations, separate from package compilation/upload.
const REGISTRY_QUERY_TIMEOUT: Duration = Duration::from_secs(30);
const OUTCOME_SCHEMA_VERSION: u32 = 1;

pub(crate) fn publish(
    publication_path: &Path,
    manifest_path: &Path,
    output: &Path,
    dry_run: bool,
    verbose: Verbose,
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
    let client = RegistryClient::new()?;
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
    let result = execute(&publication, manifest_path, &client, &mut outcome, verbose);
    if let Err(error) = result {
        // Outcomes contain only a concise handoff; full typed diagnostics stay on stderr.
        eprintln!("{error}");
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

fn execute(
    publication: &PublicationManifest,
    manifest_path: &Path,
    client: &RegistryClient,
    outcome: &mut RegistryOutcome,
    verbose: Verbose,
) -> Result<(), AppError> {
    let repository = verify_source(publication, manifest_path)?;
    let mut missing = Vec::new();
    for package in &mut outcome.packages {
        if client.contains(&package.name, &package.version)? {
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
    let session = CredentialSession::new(
        ActionsIdentity::from_environment()?,
        publication.clone(),
        source_manifest.clone(),
        target.path().to_path_buf(),
        TrustedPublisher::new()?,
    )?;
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
    let upload = command.output();
    let cleanup = session.finish();
    if let Err(error) = &cleanup {
        eprintln!("{error}");
    }
    let build_cleanup = target.close();
    if let Err(error) = &build_cleanup {
        eprintln!("Registry build-directory cleanup failed: {error}");
    }
    let upload = upload.map_err(RegistryUploadError::caused_by)?;
    eprint!("{}", String::from_utf8_lossy(&upload.stderr));
    eprint!("{}", String::from_utf8_lossy(&upload.stdout));
    observe_uploads(
        &mut outcome.packages,
        |name, version| client.contains(name, version),
        thread::sleep,
    )?;
    repository.ensure_clean_head()?;
    cleanup?;
    build_cleanup.map_err(RegistryUploadError::caused_by)?;
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
}

fn observe_uploads(
    packages: &mut [RegistryPackage],
    mut contains: impl FnMut(&str, &str) -> Result<bool, AppError>,
    mut wait: impl FnMut(Duration),
) -> Result<(), AppError> {
    // Cargo already waits on its own index view. A short shared reconciliation window
    // allows the reader's index view to catch up without waiting separately per package.
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
    if let Some(parent) = path.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|error| WriteFileError::caused_by(path, error))?;
    }
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut file =
        NamedTempFile::new_in(parent).map_err(|error| WriteFileError::caused_by(path, error))?;
    serde_json::to_writer_pretty(&mut file, outcome)
        .map_err(|error| WriteFileError::caused_by(path, error))?;
    file.persist_noclobber(path)
        .map_err(|error| WriteFileError::caused_by(path, error))?;
    Ok(())
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
#[display("registry response does not identify requested {package}@{version}")]
struct RegistryVersionMismatch {
    package: String,
    version: String,
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
    use super::*;

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
        )
        .unwrap();
        assert_eq!(result, StatusCode::OK);
        assert_eq!(waits, 2);
        query_with_retry(
            || Ok(StatusCode::FORBIDDEN),
            |status| *status,
            |_| panic!("deterministic status must not retry"),
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
