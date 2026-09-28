//! Explicit external API evidence, kept separate from offline classification and semantic decisions.

use std::env::consts::EXE_SUFFIX;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::io::{ErrorKind, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::{env, fs, io};

use crp_diag::{DiagnosticSink, Quotable as _, Stderr, Verbose, quote_path};
use crp_publication::PublicationOutput;
use crp_publication::publication::registry::RegistryClient;
use crp_versioning::inspect_plan::read_resolved_preview;
use crp_versioning::preview::Prepared;
use crp_versioning::report::{read_report, run_report_with_target};
use crp_versioning::resolved::{Inputs, ResolvedState, read_json};
use crp_versioning::semver_targets::semver_targets;
use crp_workspace::artifact_path::write_new;
use crp_workspace::command::{BUILD_CREDENTIAL_VARIABLES, run_capture};
use crp_workspace::git::GitRepo;
use crp_workspace::metadata::{MetadataJson, capture_metadata};
use ohno::AppError;
use semver::Version;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// Identifies the assessed source without accepting a report detached from its input snapshot.
enum Evidence {
    Source(Inputs),
    Preview(ResolvedState),
}

impl Evidence {
    fn root(&self) -> &Path {
        match self {
            Self::Source(inputs) => &inputs.root,
            Self::Preview(resolved) => &resolved.inputs.root,
        }
    }
    fn inputs(&self) -> &Inputs {
        match self {
            Self::Source(inputs) => inputs,
            Self::Preview(resolved) => &resolved.inputs,
        }
    }
    fn verify(&self, manifest: &Path) -> Result<(), AppError> {
        match self {
            Self::Source(inputs) => {
                inputs.verify(manifest, None)?;
            }
            Self::Preview(resolved) => resolved.verify_candidate(manifest)?,
        }
        Ok(())
    }
}

fn comparison_baseline(
    name: &str,
    parent: Option<(&str, &Path)>,
    registry: impl FnOnce(&str) -> Result<Option<Version>, AppError>,
    verbose: Verbose<'_>,
) -> Result<Option<Version>, AppError> {
    if let Some((version, root)) = parent {
        verbose.note(|| format!(
            "{name} uses anticipated parent {version} from '{}'; its final source, not a published registry version, defines this comparison.",
            quote_path(&root.to_string_lossy()),
        ));
        Ok(Some(Version::parse(version)?))
    } else {
        verbose.note(|| {
            format!("{name} retains registry baseline selection from actual release history.")
        });
        registry(name)
    }
}

/// Completed comparisons and their semantic floors, not an automatic semantic assessment.
#[derive(Debug, Serialize)]
struct CompatibilityOutcome {
    schema_version: u32,
    checker: String,
    report: PathBuf,
    completed: bool,
    findings: bool,
    packages: Vec<Comparison>,
}

impl CompatibilityOutcome {
    fn identify(&mut self, result: &Output) -> Result<(), AppError> {
        if !result.status.success() {
            return Err(CheckerFailed::new("identify itself", result.status.code()).into());
        }
        String::from_utf8_lossy(&result.stdout)
            .trim()
            .clone_into(&mut self.checker);
        Ok(())
    }

    fn assess(
        &mut self,
        targets: impl IntoIterator<Item = String>,
        mut baseline: impl FnMut(&str) -> Result<Option<Version>, AppError>,
        mut compare: impl FnMut(&str, &Version) -> Result<Output, AppError>,
        output: &mut CheckerOutput<impl Write, impl Write>,
        verbose: Verbose<'_>,
    ) -> Result<(), AppError> {
        for name in targets {
            let baseline = baseline(&name)?;
            self.compare(
                &name,
                baseline,
                |baseline| compare(&name, baseline),
                output,
                verbose,
            )?;
        }
        self.completed = true;
        Ok(())
    }

    // Acquired baselines and checker output are interpreted separately from their I/O.
    // An absent baseline cannot invoke comparison or imply a compatibility conclusion.
    fn compare(
        &mut self,
        name: &str,
        baseline: Option<Version>,
        compare: impl FnOnce(&Version) -> Result<Output, AppError>,
        output: &mut CheckerOutput<impl Write, impl Write>,
        verbose: Verbose<'_>,
    ) -> Result<(), AppError> {
        let Some(baseline) = baseline else {
            verbose.note(||format!("{name} has no available published comparison version; no compatibility conclusion is inferred."));
            self.packages.push(Comparison {
                name: name.to_owned(),
                baseline_version: None,
                required_level: None,
                compared: false,
            });
            return Ok(());
        };
        verbose.note(||format!("Comparing {name} against baseline {baseline} with all features from the captured source workspace."));
        let result = compare(&baseline)?;
        let text = output.record(&result)?;
        let floor = interpret(result.status.code(), &text)?;
        self.findings |= result.status.code() == Some(100);
        self.packages.push(Comparison {
            name: name.to_owned(),
            baseline_version: Some(baseline.to_string()),
            required_level: floor,
            compared: true,
        });
        Ok(())
    }

    fn conclusion(&self, output: &Path, deny_findings: bool) -> (bool, String) {
        (
            !deny_findings || !self.findings,
            format!(
                "{}Compatibility evidence: {}.",
                if deny_findings && self.findings {
                    format!(
                        "Insufficient version increments: {}. ",
                        self.packages
                            .iter()
                            .filter_map(|package| package
                                .required_level
                                .map(|level| format!("{} requires {level}", package.name)))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                } else {
                    String::new()
                },
                output.join("compatibility.json").display(),
            ),
        )
    }

    fn finish(
        &mut self,
        comparison: Result<(), AppError>,
        unchanged: Result<(), AppError>,
        persist: impl FnOnce(&Self) -> Result<(), AppError>,
    ) -> Result<(), AppError> {
        // Failed comparisons still need source verification and persisted incomplete evidence.
        // A source change invalidates completed evidence, but must not hide the comparison error.
        if unchanged.is_err() {
            self.completed = false;
        }
        let persistence = persist(self);
        let operation = match (comparison, unchanged) {
            (Err(comparison), Err(verification)) => {
                Err(ComparisonAndSourceFailed::caused_by(verification, comparison).into())
            }
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        };
        match (operation, persistence) {
            (Err(operation), Err(persistence)) => {
                Err(EvidencePersistenceAlsoFailed::caused_by(persistence, operation).into())
            }
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }
}

/// One contract's baseline comparison and any floor supplied by the checker.
///
/// Unavailable evidence has no baseline or floor and is not compared. Completed comparisons
/// always identify their baseline; an absent floor then means no minimum was established.
/// Ref: book/src/reference/artifacts.md, "Compatibility evidence".
#[derive(Debug, Serialize)]
struct Comparison {
    name: String,
    baseline_version: Option<String>,
    required_level: Option<&'static str>,
    compared: bool,
}

/// Current compatibility.json layout; independent of the report and plan schemas.
pub(crate) const COMPATIBILITY_SCHEMA_VERSION: u32 = 1;

/// Persists critical checker evidence while deferring secondary diagnostic-delivery failures.
struct CheckerOutput<L, M> {
    log: L,
    mirror: M,
    delivery_error: Option<AppError>,
}

impl<L: Write, M: Write> CheckerOutput<L, M> {
    fn new(log: L, mirror: M) -> Self {
        Self {
            log,
            mirror,
            delivery_error: None,
        }
    }

    fn record(&mut self, result: &Output) -> Result<String, AppError> {
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let persisted = self
            .log
            .write_all(text.as_bytes())
            .map_err(CheckerLogFailed::caused_by);
        // Once delivery fails, keep collecting the critical log rather than repeatedly writing
        // to a broken secondary stream. Its original failure is returned only after finalization.
        if self.delivery_error.is_none()
            && let Err(error) = self.mirror.write_all(text.as_bytes())
        {
            self.delivery_error = Some(CheckerMirrorFailed::caused_by(error).into());
        }
        persisted?;
        Ok(text)
    }

    fn finish(self, operation: Result<(), AppError>) -> Result<(), AppError> {
        finish_delivery(operation, self.delivery_error)
    }
}

/// Defers errors from supporting diagnostics so retry/report notes cannot unwind checker cleanup.
///
/// The compatibility invocation owns final error propagation. This adapter does not change the
/// diagnostic component's general policy or hide its failure from that final result.
#[derive(Debug)]
struct DeferredDiagnostics {
    destination: Arc<dyn DiagnosticSink>,
    failure: Mutex<Option<AppError>>,
}

impl DeferredDiagnostics {
    fn new(destination: Arc<dyn DiagnosticSink>) -> Self {
        Self {
            destination,
            failure: Mutex::new(None),
        }
    }

    fn take_failure(&self) -> Option<AppError> {
        self.failure
            .lock()
            .expect("diagnostic state has no callbacks under its lock")
            .take()
    }
}

impl DiagnosticSink for DeferredDiagnostics {
    fn write(&self, text: &str) -> io::Result<()> {
        if self
            .failure
            .lock()
            .expect("diagnostic state has no callbacks under its lock")
            .is_some()
        {
            return Ok(());
        }
        if let Err(error) = self.destination.write(text) {
            let error = CheckerMirrorFailed::caused_by(error).into();
            let mut failure = self
                .failure
                .lock()
                .expect("diagnostic state has no callbacks under its lock");
            if failure.is_none() {
                *failure = Some(error);
            } else {
                drop(failure);
                drop(error);
            }
        }
        Ok(())
    }
}

fn finish_delivery<T>(
    operation: Result<T, AppError>,
    delivery: Option<AppError>,
) -> Result<T, AppError> {
    match (operation, delivery) {
        (Err(operation), Some(delivery)) => {
            Err(CheckerDeliveryAlsoFailed::caused_by(delivery, operation).into())
        }
        (Err(error), None) | (Ok(_), Some(error)) => Err(error),
        (Ok(value), None) => Ok(value),
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Pass the explicit command options without a second application request type"
)]
pub(crate) fn check_with_target(
    manifest: &Path,
    prepared: Option<&Path>,
    plan: Option<&Path>,
    release_history: Option<&str>,
    merge_target: Option<&str>,
    output: &Path,
    deny_findings: bool,
    verbose: bool,
) -> Result<(bool, String), AppError> {
    let deferred = Arc::new(DeferredDiagnostics::new(Arc::new(Stderr)));
    let diagnostics = PublicationOutput::new(
        env!("CARGO_PKG_VERSION"),
        verbose,
        Arc::<DeferredDiagnostics>::clone(&deferred),
    );
    let result = check_with_output(
        manifest,
        prepared,
        plan,
        release_history,
        merge_target,
        output,
        deny_findings,
        &diagnostics,
    );
    finish_delivery(result, deferred.take_failure())
}

#[expect(
    clippy::too_many_arguments,
    reason = "Keep the command options together while substituting the diagnostic destination"
)]
fn check_with_output(
    manifest: &Path,
    prepared: Option<&Path>,
    plan: Option<&Path>,
    base: Option<&str>,
    merge_target: Option<&str>,
    output: &Path,
    deny_findings: bool,
    diagnostics: &PublicationOutput,
) -> Result<(bool, String), AppError> {
    let verbose = diagnostics.notes();
    if output.try_exists()? {
        return Err(CompatibilityDestinationExists::new(output).into());
    }
    fs::create_dir_all(output)?;
    let (evidence, manifest) = if let Some(path) = prepared {
        let prepared: Prepared = read_json(path)?;
        let manifest = manifest.canonicalize()?;
        prepared.inputs.verify(&manifest, None)?;
        (Evidence::Source(prepared.inputs), manifest)
    } else if let Some(path) = plan {
        let resolved = read_resolved_preview(path, manifest, verbose)?;
        let manifest = resolved.evidence_manifest_path.clone();
        (Evidence::Preview(resolved), manifest)
    } else {
        let manifest = manifest.canonicalize()?;
        let inputs = Inputs::capture_with_target(&manifest, base, merge_target)?;
        (Evidence::Source(inputs), manifest)
    };
    // Derive target selection from the bound source rather than trusting an adjacent report
    // that could have been replaced independently of the prepared/preview artifact.
    run_report_with_target(
        output,
        Some(&evidence.inputs().release_history),
        evidence.inputs().merge_target.as_deref(),
        &manifest,
        verbose,
    )?;
    evidence.verify(&manifest)?;
    let report = output.join("report.json");
    let source_report = read_report(&report)?;
    let targets = semver_targets(&source_report, verbose);
    let cache = cache_path(&env::temp_dir(), evidence.root())?;
    fs::create_dir_all(&cache)?;
    let log = output.join("semver-checks.log");
    let log = fs::File::create(log).map_err(CheckerLogFailed::caused_by)?;
    let mut checker_output = CheckerOutput::new(log, io::stderr());
    let mut outcome = CompatibilityOutcome {
        schema_version: COMPATIBILITY_SCHEMA_VERSION,
        checker: if targets.is_empty() {
            "not invoked: no consumer contracts selected"
        } else {
            "selected: checker identity unavailable"
        }
        .to_owned(),
        report,
        completed: false,
        findings: false,
        packages: Vec::new(),
    };
    let mut parent_source = None;
    let result = (|| {
        let parent_root = if let Some(anchor) = targets
            .iter()
            .find_map(|name| source_report.anticipated_parent_anchor(name))
        {
            // A caller can name a moved member. Use Cargo's workspace root, as classification
            // does, and share the single captured parent snapshot across selected contracts.
            let metadata: MetadataJson = serde_json::from_slice(&capture_metadata(&manifest)?)?;
            let workspace = GitRepo::discover(Path::new(&metadata.workspace_root))?;
            let source =
                parent_source.insert(ParentSource::create(evidence.root(), &anchor.commit)?);
            Some(source.root.join(workspace.prefix()))
        } else {
            None
        };
        let parent_baseline = |name: &str| {
            source_report.anticipated_parent_anchor(name).map(|anchor| {
                (
                    anchor.version.as_str(),
                    parent_root
                        .as_deref()
                        .expect("selected parent anchors acquired their shared source"),
                )
            })
        };
        // Source acquisition must not turn a moved named target into accepted comparison input.
        evidence.verify(&manifest)?;
        // Resolve once outside the assessed Cargo configuration. An alias in that configuration
        // must not select a different checker for identity, canary or comparison.
        let checker = if targets.is_empty() {
            None
        } else {
            Some(resolve_checker()?)
        };
        if let Some(checker) = &checker {
            let identity = invoke(checker, &manifest, &cache, &["--version"])?;
            outcome.identify(&identity)?;
            canary(checker, &cache, &mut checker_output)?;
        }
        let registry = targets
            .iter()
            .any(|name| parent_baseline(name).is_none())
            .then(|| RegistryClient::new(diagnostics.clone()))
            .transpose()?;
        outcome.assess(
            targets,
            |name| {
                comparison_baseline(
                    name,
                    parent_baseline(name),
                    |name| {
                        registry
                            .as_ref()
                            .expect("historical targets acquired the registry client")
                            .comparison_baseline(name)
                    },
                    verbose,
                )
            },
            |name, baseline| {
                evidence.verify(&manifest)?;
                let checker = checker
                    .as_deref()
                    .expect("nonempty targets resolved the checker");
                let command = match parent_baseline(name) {
                    Some((_, root)) => {
                        parent_source
                            .as_ref()
                            .expect("anticipated comparison acquired its source")
                            .verify()?;
                        source_comparison_command(checker, &manifest, name, root)
                    }
                    None => comparison_command(checker, &manifest, name, baseline),
                };
                execute_checker(command, &cache)
            },
            &mut checker_output,
            verbose,
        )
    })();
    let parent_unchanged = parent_source.as_ref().map_or(Ok(()), ParentSource::verify);
    if parent_unchanged.is_err() {
        outcome.completed = false;
    }
    let result = match (result, parent_unchanged) {
        (Err(comparison), Err(verification)) => {
            Err(ComparisonAndSourceFailed::caused_by(verification, comparison).into())
        }
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    };
    let cleanup = parent_source.as_mut().map_or(Ok(()), ParentSource::finish);
    if cleanup.is_err() {
        outcome.completed = false;
    }
    let result = finish_parent_source(result, cleanup);
    let unchanged = evidence.verify(&manifest);
    let destination = output.join("compatibility.json");
    outcome.finish(checker_output.finish(result), unchanged, |outcome| {
        write_new(&destination, |file| {
            serde_json::to_writer_pretty(file, outcome)
                .map_err(|error| CompatibilityWriteError::caused_by(&destination, error).into())
        })
    })?;
    Ok(outcome.conclusion(output, deny_findings))
}

fn comparison_command(checker: &Path, manifest: &Path, name: &str, baseline: &Version) -> Command {
    checker_command(
        checker,
        manifest,
        &[
            "--all-features",
            "--manifest-path",
            &manifest.to_string_lossy(),
            "--baseline-version",
            &baseline.to_string(),
            "-p",
            name,
        ],
    )
}

fn source_comparison_command(
    checker: &Path,
    manifest: &Path,
    name: &str,
    baseline_root: &Path,
) -> Command {
    checker_command(
        checker,
        manifest,
        &[
            "--all-features",
            "--manifest-path",
            &manifest.to_string_lossy(),
            "--baseline-root",
            &baseline_root.to_string_lossy(),
            "-p",
            name,
        ],
    )
}

/// Owns one immutable parent snapshot only for the duration of its required API comparisons.
struct ParentSource {
    directory: Option<TempDir>,
    repository: PathBuf,
    root: PathBuf,
    commit: String,
}

impl ParentSource {
    #[cfg_attr(test, mutants::skip)] // Real Git worktree acquisition and cleanup use integration tests.
    fn create(repository: &Path, commit: &str) -> Result<Self, AppError> {
        let directory = tempfile::Builder::new()
            .prefix("crp-semver-parent-")
            .tempdir()
            .map_err(|error| {
                ParentSourceFailed::caused_by(commit.to_owned(), "create source directory", error)
            })?;
        let mut source = Self {
            root: directory.path().join("source"),
            directory: Some(directory),
            repository: repository.to_owned(),
            commit: commit.to_owned(),
        };
        let created = run_capture(
            "git",
            &[
                "worktree",
                "add",
                "--detach",
                &source.root.to_string_lossy(),
                commit,
            ],
            repository,
        )
        .map_err(|error| {
            ParentSourceFailed::caused_by(commit.to_owned(), "create source worktree", error).into()
        });
        if let Err(error) = created {
            return finish_parent_source(Err(error), source.finish());
        }
        Ok(source)
    }

    #[cfg_attr(test, mutants::skip)] // Checks the pinned Git source, without resolution or compilation.
    fn verify(&self) -> Result<(), AppError> {
        let actual = GitRepo::discover(&self.root)?.rev_parse("HEAD^{commit}")?;
        if actual != self.commit {
            return Err(ParentSourceFailed::new(
                self.commit.clone(),
                "retain immutable source HEAD",
            )
            .into());
        }
        run_capture("git", &["diff", "--exit-code", "HEAD", "--"], &self.root).map_err(
            |error| {
                ParentSourceFailed::caused_by(
                    self.commit.clone(),
                    "retain unchanged source files",
                    error,
                )
            },
        )?;
        Ok(())
    }

    #[cfg_attr(test, mutants::skip)] // Git and directory cleanup preserve their independent failures.
    fn finish(&mut self) -> Result<(), AppError> {
        let Some(directory) = self.directory.take() else {
            return Ok(());
        };
        let removed = run_capture(
            "git",
            &[
                "worktree",
                "remove",
                "--force",
                &self.root.to_string_lossy(),
            ],
            &self.repository,
        )
        .map(|_| ())
        .map_err(|error| ParentSourceCleanupFailed::caused_by(&self.root, error).into());
        let closed = directory
            .close()
            .map_err(|error| ParentSourceCleanupFailed::caused_by(&self.root, error).into());
        finish_parent_source(removed, closed)
    }
}

impl Drop for ParentSource {
    #[cfg_attr(test, mutants::skip)] // Unwind fallback; normal execution explicitly finalizes.
    fn drop(&mut self) {
        if let Err(error) = self.finish() {
            // A failed diagnostic must not replace the unwind that caused fallback cleanup.
            drop(writeln!(io::stderr().lock(), "{error}"));
        }
    }
}

fn finish_parent_source<T>(
    operation: Result<T, AppError>,
    cleanup: Result<(), AppError>,
) -> Result<T, AppError> {
    match (operation, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(operation), Err(cleanup)) => {
            Err(ParentCleanupAlsoFailed::caused_by(cleanup, operation).into())
        }
    }
}

fn cache_path(temporary: &Path, root: &Path) -> Result<PathBuf, AppError> {
    // A stable short per-workspace root shares compiler artifacts across prepare/preview/verify
    // without exposing cargo-semver-checks' long generated paths to the Windows path limit.
    // The truncation is a practical compact cache namespace, not an integrity or authorization
    // identity. Cargo still validates cached build inputs; this is not a measured path limit.
    const ID_BYTES: usize = 8;
    let digest = Sha256::digest(root.as_os_str().as_encoded_bytes());
    let mut id = String::new();
    for byte in digest.iter().take(ID_BYTES) {
        write!(id, "{byte:02x}")?;
    }
    Ok(temporary.join(format!("crp-semver-{id}")))
}

fn invoke(
    checker: &Path,
    manifest: &Path,
    cache: &Path,
    args: &[&str],
) -> Result<Output, AppError> {
    execute_checker(checker_command(checker, manifest, args), cache)
}

fn checker_command(checker: &Path, manifest: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(checker);
    command
        .arg("semver-checks")
        .args(args)
        .current_dir(manifest.parent().unwrap_or_else(|| Path::new(".")));
    command
}

fn resolve_checker() -> Result<PathBuf, AppError> {
    let paths = env::var_os("PATH").ok_or_else(CheckerUnavailable::new)?;
    let filename = format!("cargo-semver-checks{EXE_SUFFIX}");
    for directory in env::split_paths(&paths) {
        let candidate = directory.join(&filename);
        let metadata = match fs::metadata(&candidate) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(CheckerLocationFailed::caused_by(&candidate, error).into()),
        };
        if !metadata.is_file() {
            continue;
        }
        #[cfg(unix)]
        {
            if metadata.permissions().mode() & 0o111 == 0 {
                continue;
            }
        }
        return candidate
            .canonicalize()
            .map_err(|error| CheckerLocationFailed::caused_by(&candidate, error).into());
    }
    Err(CheckerUnavailable::new().into())
}

// Compatibility compilation needs source-download access, not inherited upload or OIDC tokens.
// This controls the child environment, not same-user filesystem access or build-code isolation.
fn strip_checker_credentials(command: &mut Command, names: impl Iterator<Item = OsString>) {
    for name in BUILD_CREDENTIAL_VARIABLES {
        command.env_remove(name);
    }
    for name in names {
        if is_registry_token(&name) {
            command.env_remove(name);
        }
    }
}

fn is_registry_token(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        let name = name.to_ascii_uppercase();
        name == "CARGO_REGISTRY_TOKEN"
            || (name.starts_with("CARGO_REGISTRIES_") && name.ends_with("_TOKEN"))
    })
}

fn execute_checker(mut command: Command, cache: &Path) -> Result<Output, AppError> {
    // Windows environment-key ordering calls the OS even before a process starts,
    // so environment configuration belongs to this native execution boundary.
    command
        .env("CARGO_TARGET_DIR", cache)
        .env("CARGO_TERM_COLOR", "never");
    strip_checker_credentials(&mut command, env::vars_os().map(|(name, _)| name));
    command
        .output()
        .map_err(|error| CheckerStartFailed::caused_by(error).into())
}

fn canary(
    checker: &Path,
    cache: &Path,
    output: &mut CheckerOutput<impl Write, impl Write>,
) -> Result<(), AppError> {
    let fixture = tempfile::Builder::new()
        .prefix("crp-semver-canary-")
        .tempdir()?;
    fs::write(
        fixture.path().join("Cargo.toml"),
        "[package]\nname='release-plan-canary'\nversion='1.0.0'\nedition='2024'\n[lib]\npath='lib.rs'\n[workspace]\n",
    )?;
    fs::write(fixture.path().join("lib.rs"), "pub fn canary() {}\n")?;
    let manifest = fixture.path().join("Cargo.toml");
    let result = invoke(
        checker,
        &manifest,
        cache,
        &[
            "--manifest-path",
            &manifest.to_string_lossy(),
            "--baseline-root",
            &fixture.path().to_string_lossy(),
            "--all-features",
        ],
    )?;
    let text = output.record(&result)?;
    validate_canary(result.status.code(), &text)
}

fn validate_canary(code: Option<i32>, text: &str) -> Result<(), AppError> {
    if code != Some(0) {
        return Err(CheckerFailed::new("complete its identical-source canary", code).into());
    }
    if interpret(code, text)?.is_some() {
        return Err(CanaryComparisonChanged::new().into());
    }
    Ok(())
}

// The supported integration is tested against CARGO_SEMVER_CHECKS_VERSION in constants.env.
// cargo-semver-checks v0.50.0 main.rs::check_exit_code and check_release.rs::print_report
// supply these statuses and summaries; its CLI does not offer a stable structured report.
// Keep fixtures and the canary aligned when updating the pin. Unknown output is an operational
// failure, never missing evidence interpreted as compatibility.
fn interpret(code: Option<i32>, text: &str) -> Result<Option<&'static str>, AppError> {
    if !matches!(code, Some(0 | 100)) {
        return Err(CheckerFailed::new("complete the comparison", code).into());
    }
    if text
        .lines()
        .any(|line| line.contains("Summary semver requires new major version"))
    {
        return Ok(Some("breaking"));
    }
    if text
        .lines()
        .any(|line| line.contains("Summary semver requires new minor version"))
    {
        return Ok(Some("nonbreaking"));
    }
    if code == Some(0)
        && text
            .lines()
            .any(|line| line.contains("Summary no semver update required"))
    {
        return Ok(None);
    }
    Err(CheckerSummaryMissing::new().into())
}

/// Identifies a caller-selected destination that cannot hold a fresh evidence attempt.
#[ohno::error]
#[display("compatibility output directory must be new: '{}'", path.quoted())]
struct CompatibilityDestinationExists {
    path: PathBuf,
}

/// Retains the process-start cause separately from completed checker failures.
#[ohno::error]
#[display("failed to start cargo-semver-checks")]
struct CheckerStartFailed;

/// An installed checker could not be located without consulting candidate Cargo aliases.
#[ohno::error]
#[display("cargo-semver-checks executable is unavailable in PATH")]
struct CheckerUnavailable;

/// Preserves the failed filesystem observation while locating the checker.
#[ohno::error]
#[display("cannot inspect checker executable '{}'", path.quoted())]
struct CheckerLocationFailed {
    path: PathBuf,
}

/// Identifies which checker operation failed and its observed exit status.
#[ohno::error]
#[display("cargo-semver-checks failed to {operation} with status {status:?}")]
struct CheckerFailed {
    operation: &'static str,
    status: Option<i32>,
}

/// A successful process exit alone does not establish that comparison completed.
#[ohno::error]
#[display("compatibility checker supplied no recognized complete summary")]
struct CheckerSummaryMissing;

/// Failure to retain the critical checker log prevents accepting a comparison.
#[ohno::error]
#[display("failed to write the compatibility checker log")]
struct CheckerLogFailed;

/// A secondary output stream failed after the checker output was captured.
#[ohno::error]
#[display("failed to mirror compatibility checker diagnostics")]
struct CheckerMirrorFailed;

/// Retains secondary delivery failure alongside an independent checker or log failure.
#[ohno::error]
#[display("checker diagnostic delivery also failed: {delivery}")]
struct CheckerDeliveryAlsoFailed {
    delivery: AppError,
}

/// The identical-source canary cannot establish a semantic change requirement.
#[ohno::error]
#[display("compatibility checker reported a change in its identical-source canary")]
struct CanaryComparisonChanged;

/// Keeps source-invalidity diagnostics alongside the original comparison failure.
#[ohno::error]
#[display("source verification also failed: {verification}")]
struct ComparisonAndSourceFailed {
    verification: AppError,
}

/// Preserves an artifact-write failure alongside prior comparison or source failures.
#[ohno::error]
#[display("compatibility evidence persistence also failed: {persistence}")]
struct EvidencePersistenceAlsoFailed {
    persistence: AppError,
}

/// Identifies failure to serialize the application's compatibility evidence.
#[ohno::error]
#[display("Failed to write '{}'", path.quoted())]
struct CompatibilityWriteError {
    path: PathBuf,
}

/// Identifies the fixed parent snapshot whose acquisition failed.
#[ohno::error]
#[display("cannot {operation} for anticipated parent {commit}")]
struct ParentSourceFailed {
    commit: String,
    operation: &'static str,
}

/// Retains the failed owned source cleanup operation and its original cause.
#[ohno::error]
#[display("cannot clean up compatibility parent source '{}'", path.quoted())]
struct ParentSourceCleanupFailed {
    path: PathBuf,
}

/// A cleanup failure cannot erase an independent checker/acquisition failure.
#[ohno::error]
#[display("anticipated-parent source cleanup also failed: {cleanup}")]
struct ParentCleanupAlsoFailed {
    cleanup: AppError,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::Cell;
    use std::ffi::OsStr;
    use std::io::Error;
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt;
    use std::process::ExitStatus;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    use super::*;

    fn outcome() -> CompatibilityOutcome {
        CompatibilityOutcome {
            schema_version: COMPATIBILITY_SCHEMA_VERSION,
            checker: "not invoked".to_owned(),
            report: PathBuf::from("report.json"),
            completed: false,
            findings: false,
            packages: Vec::new(),
        }
    }

    fn checker_output(code: u8, stdout: &[u8], stderr: &[u8]) -> Output {
        // Unix from_raw consumes a wait status, whose exit-code field is the shifted byte.
        #[cfg(unix)]
        let status = ExitStatus::from_raw(i32::from(code) << 8);
        #[cfg(windows)]
        let status = ExitStatus::from_raw(u32::from(code));
        Output {
            status,
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
        }
    }

    #[test]
    fn compiler_cache_is_stable_per_original_workspace() {
        let root = Path::new("workspace");
        let temporary = Path::new("temporary");
        assert_eq!(
            cache_path(temporary, root).unwrap(),
            cache_path(temporary, root).unwrap()
        );
        assert_ne!(
            cache_path(temporary, root).unwrap(),
            cache_path(temporary, Path::new("another")).unwrap()
        );
    }

    #[test]
    fn findings_and_execution_failure_are_distinct() {
        assert_eq!(
            interpret(Some(0), " Summary no semver update required").unwrap(),
            None
        );
        assert_eq!(
            interpret(Some(100), " Summary semver requires new minor version: 1").unwrap(),
            Some("nonbreaking")
        );
        assert_eq!(
            interpret(Some(100), " Summary semver requires new major version: 1").unwrap(),
            Some("breaking")
        );
        for code in [None, Some(1), Some(2)] {
            interpret(code, " Summary no semver update required").unwrap_err();
        }
        interpret(Some(0), "checker did not compare").unwrap_err();
        interpret(Some(100), " Summary no semver update required").unwrap_err();
    }

    #[test]
    fn checker_identity_requires_success_and_preserves_its_reported_version() {
        let mut outcome = outcome();
        outcome
            .identify(&checker_output(1, b"partial identity", b"execution failed"))
            .unwrap_err();
        assert_eq!(outcome.checker, "not invoked");
        outcome
            .identify(&checker_output(
                0,
                b"cargo-semver-checks 0.50.0\n",
                b"diagnostic",
            ))
            .unwrap();
        assert_eq!(outcome.checker, "cargo-semver-checks 0.50.0");
    }

    #[test]
    fn missing_baseline_records_no_conclusion_and_never_runs_the_checker() {
        let mut outcome = outcome();
        let mut log = Vec::new();
        outcome
            .compare(
                "new-library",
                None,
                |_| panic!("an unpublished package cannot be compared"),
                &mut CheckerOutput::new(&mut log, io::sink()),
                Verbose::new(true, &crp_diag::Discard),
            )
            .unwrap();
        assert!(!outcome.findings);
        assert!(log.is_empty());
        assert_eq!(
            serde_json::to_value(&outcome.packages).unwrap(),
            json!([{
                "name":"new-library", "baseline_version":null,
                "required_level":null, "compared":false
            }])
        );
        assert!(outcome.conclusion(Path::new("evidence"), true).0);
    }

    #[test]
    fn completed_comparisons_keep_floors_separate_from_increment_findings() {
        for code in [0, 100] {
            let mut outcome = outcome();
            let mut log = Vec::new();
            for (name, summary, floor) in [
                (
                    "breaking-library",
                    " Summary semver requires new major version: 1\n",
                    "breaking",
                ),
                (
                    "extended-library",
                    " Summary semver requires new minor version: 1\n",
                    "nonbreaking",
                ),
            ] {
                outcome
                    .compare(
                        name,
                        Some(Version::new(1, 2, 3)),
                        |baseline| {
                            assert_eq!(baseline, &Version::new(1, 2, 3));
                            Ok(checker_output(code, b"comparison\n", summary.as_bytes()))
                        },
                        &mut CheckerOutput::new(&mut log, io::sink()),
                        Verbose::new(true, &crp_diag::Discard),
                    )
                    .unwrap();
                let comparison = outcome.packages.last().unwrap();
                assert_eq!(comparison.name, name);
                assert_eq!(comparison.baseline_version.as_deref(), Some("1.2.3"));
                assert_eq!(comparison.required_level, Some(floor));
                assert!(comparison.compared);
            }
            outcome
                .compare(
                    "unchanged-library",
                    Some(Version::new(1, 2, 3)),
                    |_| {
                        Ok(checker_output(
                            0,
                            b" Summary no semver update required\n",
                            b"",
                        ))
                    },
                    &mut CheckerOutput::new(&mut log, io::sink()),
                    Verbose::new(false, &crp_diag::Discard),
                )
                .unwrap();
            assert!(outcome.packages.last().unwrap().required_level.is_none());
            assert_eq!(outcome.findings, code == 100);
            assert!(outcome.conclusion(Path::new("evidence"), false).0);
            let (passed, message) = outcome.conclusion(Path::new("evidence"), true);
            assert_eq!(passed, code == 0);
            if code == 100 {
                assert!(message.contains("breaking-library requires breaking"));
                assert!(message.contains("extended-library requires nonbreaking"));
                assert!(!message.contains("unchanged-library requires"));
            }
            assert!(
                message.contains(
                    &Path::new("evidence")
                        .join("compatibility.json")
                        .display()
                        .to_string()
                )
            );
            assert!(
                String::from_utf8(log)
                    .unwrap()
                    .contains("comparison\n Summary")
            );
        }
    }

    #[test]
    fn failed_or_incomplete_comparisons_keep_logs_without_inventing_results() {
        let mut outcome = outcome();
        let mut log = Vec::new();
        outcome
            .compare(
                "completed-library",
                Some(Version::new(1, 0, 0)),
                |_| {
                    Ok(checker_output(
                        0,
                        b" Summary no semver update required\n",
                        b"",
                    ))
                },
                &mut CheckerOutput::new(&mut log, io::sink()),
                Verbose::new(false, &crp_diag::Discard),
            )
            .unwrap();
        let completed = serde_json::to_value(&outcome.packages).unwrap();
        for (code, text) in [
            (1, "compiler failed"),
            (0, "comparison was interrupted"),
            (100, " Summary no semver update required"),
        ] {
            outcome
                .compare(
                    "library",
                    Some(Version::new(1, 0, 0)),
                    |_| Ok(checker_output(code, text.as_bytes(), b"")),
                    &mut CheckerOutput::new(&mut log, io::sink()),
                    Verbose::new(false, &crp_diag::Discard),
                )
                .unwrap_err();
            assert_eq!(serde_json::to_value(&outcome.packages).unwrap(), completed);
            assert!(!outcome.findings);
            assert!(log.ends_with(text.as_bytes()));
        }
        let previous = log.clone();
        outcome
            .compare(
                "library",
                Some(Version::new(1, 0, 0)),
                |_| Err(Error::other("unavailable checker").into()),
                &mut CheckerOutput::new(&mut log, io::sink()),
                Verbose::new(false, &crp_diag::Discard),
            )
            .unwrap_err();
        assert_eq!(log, previous);
        assert_eq!(serde_json::to_value(&outcome.packages).unwrap(), completed);
    }

    #[test]
    fn assessment_completes_only_after_every_selected_contract_is_processed() {
        let mut outcome = outcome();
        let mut queried = Vec::new();
        let mut compared = Vec::new();
        outcome
            .assess(
                vec!["new".to_owned(), "published".to_owned()],
                |name| {
                    queried.push(name.to_owned());
                    Ok((name == "published").then_some(Version::new(1, 0, 0)))
                },
                |name, baseline| {
                    compared.push((name.to_owned(), baseline.clone()));
                    Ok(checker_output(
                        100,
                        b" Summary semver requires new major version: 1\n",
                        b"",
                    ))
                },
                &mut CheckerOutput::new(Vec::new(), io::sink()),
                Verbose::new(true, &crp_diag::Discard),
            )
            .unwrap();
        assert_eq!(queried, ["new", "published"]);
        assert_eq!(compared, [("published".to_owned(), Version::new(1, 0, 0))]);
        assert!(outcome.completed);
        assert!(outcome.findings);
        assert_eq!(
            serde_json::to_value(&outcome.packages).unwrap(),
            json!([
                {"name":"new","baseline_version":null,"required_level":null,"compared":false},
                {"name":"published","baseline_version":"1.0.0","required_level":"breaking","compared":true}
            ])
        );
    }

    #[test]
    fn failed_assessment_retains_completed_packages_and_stops_further_work() {
        for fail_query in [false, true] {
            let mut outcome = outcome();
            let mut queried = Vec::new();
            let mut compared = Vec::new();
            outcome
                .assess(
                    vec!["first".to_owned(), "failed".to_owned(), "later".to_owned()],
                    |name| {
                        queried.push(name.to_owned());
                        if fail_query && name == "failed" {
                            return Err(Error::other("query failed").into());
                        }
                        Ok(Some(Version::new(1, 0, 0)))
                    },
                    |name, _| {
                        compared.push(name.to_owned());
                        if name == "failed" {
                            return Ok(checker_output(1, b"", b"comparison failed"));
                        }
                        Ok(checker_output(
                            0,
                            b" Summary no semver update required",
                            b"",
                        ))
                    },
                    &mut CheckerOutput::new(Vec::new(), io::sink()),
                    Verbose::new(false, &crp_diag::Discard),
                )
                .unwrap_err();
            assert_eq!(queried, ["first", "failed"]);
            assert_eq!(
                compared,
                if fail_query {
                    vec!["first"]
                } else {
                    vec!["first", "failed"]
                }
            );
            assert!(!outcome.completed);
            assert!(!outcome.findings);
            assert_eq!(
                serde_json::to_value(&outcome.packages).unwrap(),
                json!([{"name":"first","baseline_version":"1.0.0","required_level":null,"compared":true}])
            );
        }
    }

    #[test]
    fn failed_logging_cannot_promote_a_comparison_to_evidence() {
        let mut outcome = outcome();
        let mut full_log = &mut [][..];
        outcome
            .compare(
                "library",
                Some(Version::new(1, 0, 0)),
                |_| {
                    Ok(checker_output(
                        0,
                        b" Summary no semver update required",
                        b"",
                    ))
                },
                &mut CheckerOutput::new(&mut full_log, io::sink()),
                Verbose::new(false, &crp_diag::Discard),
            )
            .unwrap_err();
        assert!(outcome.packages.is_empty());
        assert!(!outcome.findings);
    }

    #[test]
    fn supporting_diagnostic_failures_are_deferred_until_evidence_finalization() {
        let destination = Arc::new(ClosedDiagnostics(AtomicUsize::new(0)));
        let deferred = Arc::new(DeferredDiagnostics::new(Arc::<ClosedDiagnostics>::clone(
            &destination,
        )));
        let diagnostics = PublicationOutput::new(
            "fixture-version",
            true,
            Arc::<DeferredDiagnostics>::clone(&deferred),
        );
        diagnostics.line(format_args!("registry retry"));
        diagnostics.notes().note(|| "later comparison".to_owned());
        assert_eq!(destination.0.load(Ordering::Relaxed), 1);
        let persisted = Cell::new(false);
        let result = outcome().finish(
            Err(ComparisonFailure::new().into()),
            Err(SourceFailure::new().into()),
            |_| {
                persisted.set(true);
                Err(PersistenceFailure::new().into())
            },
        );
        let error = finish_delivery(result, deferred.take_failure()).unwrap_err();
        assert!(persisted.get());
        let text = error.to_string();
        for marker in [
            "comparison canary",
            "source canary",
            "persistence canary",
            "supporting mirror canary",
        ] {
            assert!(text.contains(marker));
        }
        assert!(deferred.take_failure().is_none());
    }

    /// Counts attempted delivery to an unavailable supporting diagnostic destination.
    #[derive(Debug)]
    struct ClosedDiagnostics(AtomicUsize);

    impl DiagnosticSink for ClosedDiagnostics {
        fn write(&self, _text: &str) -> io::Result<()> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Err(Error::other("supporting mirror canary"))
        }
    }

    #[test]
    fn mirror_failure_does_not_interrupt_comparisons_or_critical_logging() {
        let mut outcome = outcome();
        let mut output = CheckerOutput::new(Vec::new(), TestWriter::failing("mirror canary"));
        let result = outcome.assess(
            vec!["first".to_owned(), "second".to_owned()],
            |_| Ok(Some(Version::new(1, 0, 0))),
            |name, _| {
                Ok(checker_output(
                    0,
                    name.as_bytes(),
                    b"\n Summary no semver update required\n",
                ))
            },
            &mut output,
            Verbose::new(false, &crp_diag::Discard),
        );
        result.as_ref().unwrap();
        assert!(outcome.completed);
        assert_eq!(outcome.packages.len(), 2);
        let log = String::from_utf8(output.log.clone()).unwrap();
        assert!(log.contains("first"));
        assert!(log.contains("second"));
        assert_eq!(output.mirror.writes, 1);
        let persisted = Cell::new(false);
        let error = outcome
            .finish(output.finish(result), Ok(()), |outcome| {
                assert!(outcome.completed);
                assert_eq!(outcome.packages.len(), 2);
                persisted.set(true);
                Ok(())
            })
            .unwrap_err();
        assert!(persisted.get());
        assert!(error.find_source::<CheckerMirrorFailed>().is_some());
    }

    #[test]
    fn finalization_retains_log_or_checker_mirror_source_and_persistence_failures() {
        for log_fails in [false, true] {
            for source_fails in [false, true] {
                for persistence_fails in [false, true] {
                    let mut outcome = outcome();
                    let mut output = CheckerOutput::new(
                        TestWriter {
                            fails: log_fails,
                            ..TestWriter::failing("log canary")
                        },
                        TestWriter::failing("mirror canary"),
                    );
                    let result = outcome.compare(
                        "library",
                        Some(Version::new(1, 0, 0)),
                        |_| {
                            Ok(checker_output(
                                1,
                                b"captured stdout\n",
                                b"captured stderr\n",
                            ))
                        },
                        &mut output,
                        Verbose::new(false, &crp_diag::Discard),
                    );
                    result.as_ref().unwrap_err();
                    assert!(outcome.packages.is_empty());
                    let persisted = Cell::new(false);
                    let error = outcome
                        .finish(
                            output.finish(result),
                            if source_fails {
                                Err(SourceFailure::new().into())
                            } else {
                                Ok(())
                            },
                            |outcome| {
                                assert!(!outcome.completed);
                                persisted.set(true);
                                if persistence_fails {
                                    Err(PersistenceFailure::new().into())
                                } else {
                                    Ok(())
                                }
                            },
                        )
                        .unwrap_err();
                    assert!(persisted.get());
                    assert_eq!(error.find_source::<CheckerLogFailed>().is_some(), log_fails);
                    assert_eq!(error.find_source::<CheckerFailed>().is_some(), !log_fails);
                    let delivery = error.find_source::<CheckerDeliveryAlsoFailed>().unwrap();
                    assert!(
                        delivery
                            .delivery
                            .find_source::<CheckerMirrorFailed>()
                            .is_some()
                    );
                    let diagnostic = error.to_string();
                    assert!(diagnostic.contains("mirror canary"));
                    assert_eq!(diagnostic.contains("log canary"), log_fails);
                    assert_eq!(diagnostic.contains("source canary"), source_fails);
                    assert_eq!(diagnostic.contains("persistence canary"), persistence_fails);
                }
            }
        }
    }

    /// Records successful writes or injects one attributable output failure without OS I/O.
    struct TestWriter {
        fails: bool,
        marker: &'static str,
        writes: usize,
        bytes: Vec<u8>,
    }

    impl TestWriter {
        fn failing(marker: &'static str) -> Self {
            Self {
                fails: true,
                marker,
                writes: 0,
                bytes: Vec::new(),
            }
        }
    }

    impl Write for TestWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.writes = self.writes.checked_add(1).unwrap();
            if self.fails {
                Err(Error::other(self.marker))
            } else {
                self.bytes.extend_from_slice(buf);
                Ok(buf.len())
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn finish_preserves_comparison_source_and_persistence_failures() {
        for comparison_fails in [false, true] {
            for source_fails in [false, true] {
                for persistence_fails in [false, true] {
                    let mut outcome = outcome();
                    outcome.completed = !comparison_fails;
                    let persisted = Cell::new(false);
                    let result = outcome.finish(
                        if comparison_fails {
                            Err(ComparisonFailure::new().into())
                        } else {
                            Ok(())
                        },
                        if source_fails {
                            Err(SourceFailure::new().into())
                        } else {
                            Ok(())
                        },
                        |outcome| {
                            assert_eq!(outcome.completed, !comparison_fails && !source_fails);
                            assert_eq!(
                                serde_json::to_value(outcome)
                                    .unwrap()
                                    .get("schema_version")
                                    .unwrap(),
                                COMPATIBILITY_SCHEMA_VERSION
                            );
                            persisted.set(true);
                            if persistence_fails {
                                Err(PersistenceFailure::new().into())
                            } else {
                                Ok(())
                            }
                        },
                    );
                    assert!(persisted.get());
                    if comparison_fails || source_fails || persistence_fails {
                        let error = result.unwrap_err();
                        let diagnostic = error.to_string();
                        for (failed, marker) in [
                            (comparison_fails, "comparison canary"),
                            (source_fails, "source canary"),
                            (persistence_fails, "persistence canary"),
                        ] {
                            assert_eq!(diagnostic.contains(marker), failed);
                        }
                        if persistence_fails && (comparison_fails || source_fails) {
                            let combined = error
                                .find_source::<EvidencePersistenceAlsoFailed>()
                                .unwrap();
                            assert!(
                                combined
                                    .persistence
                                    .find_source::<PersistenceFailure>()
                                    .is_some()
                            );
                        } else {
                            assert_eq!(
                                error.find_source::<PersistenceFailure>().is_some(),
                                persistence_fails
                            );
                        }
                        assert_eq!(
                            error.find_source::<ComparisonFailure>().is_some(),
                            comparison_fails
                        );
                        if comparison_fails && source_fails {
                            let combined =
                                error.find_source::<ComparisonAndSourceFailed>().unwrap();
                            assert!(
                                combined
                                    .verification
                                    .find_source::<SourceFailure>()
                                    .is_some()
                            );
                            let diagnostic = error.to_string();
                            assert!(diagnostic.contains("comparison canary"));
                            assert!(diagnostic.contains("source canary"));
                        } else {
                            assert_eq!(
                                error.find_source::<SourceFailure>().is_some(),
                                source_fails
                            );
                        }
                    } else {
                        result.unwrap();
                    }
                }
            }
        }
    }

    #[test]
    fn canary_requires_a_recognized_successful_unchanged_comparison() {
        validate_canary(Some(0), " Summary no semver update required").unwrap();
        for code in [None, Some(1), Some(100)] {
            let error = validate_canary(code, " Summary no semver update required").unwrap_err();
            let failure = error.find_source::<CheckerFailed>().unwrap();
            assert_eq!(failure.status, code);
        }
        let error = validate_canary(Some(0), "version only").unwrap_err();
        assert!(error.find_source::<CheckerSummaryMissing>().is_some());
        let error =
            validate_canary(Some(0), " Summary semver requires new major version").unwrap_err();
        assert!(error.find_source::<CanaryComparisonChanged>().is_some());
    }

    /// Independent failures distinguish the two retained diagnostic paths.
    #[ohno::error]
    #[display("comparison canary")]
    struct ComparisonFailure;

    /// Source invalidation remains an error even when the comparison also failed.
    #[ohno::error]
    #[display("source canary")]
    struct SourceFailure;

    /// A separate write failure must not erase the operation that required the evidence.
    #[ohno::error]
    #[display("persistence canary")]
    struct PersistenceFailure;

    #[test]
    fn registry_upload_token_family_excludes_noncredential_configuration() {
        for name in [
            "CARGO_REGISTRY_TOKEN",
            "CARGO_REGISTRIES_CRATES_IO_TOKEN",
            "CARGO_REGISTRIES_PRIVATE_TOKEN",
            "cargo_registries_private_token",
        ] {
            assert!(is_registry_token(OsStr::new(name)));
        }
        for name in [
            "CARGO_REGISTRIES_PRIVATE_INDEX",
            "CARGO_HOME",
            "RUSTUP_TOOLCHAIN",
        ] {
            assert!(!is_registry_token(OsStr::new(name)));
        }
    }

    #[test]
    fn checker_arguments_bind_package_baseline_features_and_source() {
        let manifest = Path::new("candidate").join("Cargo.toml");
        let checker = Path::new("installed-checker");
        let command = comparison_command(checker, &manifest, "library", &Version::new(1, 2, 3));
        assert_eq!(command.get_program(), checker);
        assert_eq!(command.get_current_dir(), Some(Path::new("candidate")));
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(
            args,
            [
                OsStr::new("semver-checks"),
                OsStr::new("--all-features"),
                OsStr::new("--manifest-path"),
                manifest.as_os_str(),
                OsStr::new("--baseline-version"),
                OsStr::new("1.2.3"),
                OsStr::new("-p"),
                OsStr::new("library"),
            ]
        );
        assert_eq!(
            checker_command(checker, Path::new(""), &["--version"]).get_current_dir(),
            Some(Path::new("."))
        );
    }

    #[test]
    fn anticipated_parent_comparison_uses_source_instead_of_a_registry_version() {
        let manifest = Path::new("candidate").join("Cargo.toml");
        let baseline = Path::new("parent").join("nested-workspace");
        let command =
            source_comparison_command(Path::new("checker"), &manifest, "library", &baseline);
        assert_eq!(command.get_current_dir(), Some(Path::new("candidate")));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            [
                OsStr::new("semver-checks"),
                OsStr::new("--all-features"),
                OsStr::new("--manifest-path"),
                manifest.as_os_str(),
                OsStr::new("--baseline-root"),
                baseline.as_os_str(),
                OsStr::new("-p"),
                OsStr::new("library"),
            ]
        );
    }

    #[test]
    fn anticipated_anchor_bypasses_registry_baseline_selection() {
        let verbose = Verbose::new(false, &crp_diag::Discard);
        assert_eq!(
            comparison_baseline(
                "library",
                Some(("1.1.0", Path::new("parent"))),
                |_| panic!("an anticipated anchor must not read the registry"),
                verbose
            )
            .unwrap(),
            Some(Version::new(1, 1, 0))
        );
        assert_eq!(
            comparison_baseline(
                "historical",
                None,
                |name| {
                    assert_eq!(name, "historical");
                    Ok(Some(Version::new(1, 0, 0)))
                },
                verbose
            )
            .unwrap(),
            Some(Version::new(1, 0, 0))
        );
    }

    #[test]
    fn parent_cleanup_preserves_both_comparison_and_cleanup_failures() {
        let error = finish_parent_source::<()>(
            Err(CheckerSummaryMissing::new().into()),
            Err(ParentSourceCleanupFailed::new(Path::new("parent-source")).into()),
        )
        .unwrap_err();
        assert!(error.find_source::<CheckerSummaryMissing>().is_some());
        assert!(
            error
                .find_source::<ParentCleanupAlsoFailed>()
                .unwrap()
                .cleanup
                .find_source::<ParentSourceCleanupFailed>()
                .is_some()
        );
        let error = finish_parent_source(
            Ok(()),
            Err(ParentSourceCleanupFailed::new(Path::new("parent-source")).into()),
        )
        .unwrap_err();
        assert!(error.find_source::<ParentSourceCleanupFailed>().is_some());
    }

    #[test]
    fn record_keeps_both_streams_readable_when_checker_output_is_not_utf8() {
        let mut log = Vec::new();
        let text = CheckerOutput::new(&mut log, io::sink())
            .record(&checker_output(1, b"stdout\xff", b"stderr\n"))
            .unwrap();
        assert_eq!(text, "stdout\u{fffd}stderr\n");
        assert_eq!(log, text.as_bytes());
    }
}
