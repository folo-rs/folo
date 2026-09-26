//! Explicit external API evidence, kept separate from offline classification and semantic decisions.

use std::fmt::Write as _;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::{env, fs};

use crp_diag::{Quotable as _, Verbose};
use crp_publication::PublicationOutput;
use crp_publication::publication::registry::RegistryClient;
use crp_versioning::inspect_plan::read_resolved_preview;
use crp_versioning::preview::Prepared;
use crp_versioning::report::{read_report, run_report};
use crp_versioning::resolved::{Inputs, ResolvedState, read_json};
use crp_versioning::semver_targets::semver_targets;
use crp_workspace::artifact_path::write_new;
use ohno::AppError;
use semver::Version;
use serde::Serialize;
use sha2::{Digest, Sha256};

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
    fn base(&self) -> &str {
        match self {
            Self::Source(inputs) => &inputs.base,
            Self::Preview(resolved) => &resolved.inputs.base,
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
        log: &mut impl Write,
        verbose: Verbose<'_>,
    ) -> Result<(), AppError> {
        for name in targets {
            let baseline = baseline(&name)?;
            self.compare(
                &name,
                baseline,
                |baseline| compare(&name, baseline),
                log,
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
        log: &mut impl Write,
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
        verbose.note(||format!("Comparing {name} against published {baseline} with all features from the captured source workspace."));
        let result = compare(&baseline)?;
        let text = record(&result, log)?;
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
        persist(self)?;
        match (comparison, unchanged) {
            (Err(comparison), Err(verification)) => {
                Err(ComparisonAndSourceFailed::caused_by(verification, comparison).into())
            }
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }
}

/// One contract's published comparison and any floor supplied by the checker.
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
const COMPATIBILITY_SCHEMA_VERSION: u32 = 1;

pub(crate) fn check(
    manifest: &Path,
    prepared: Option<&Path>,
    plan: Option<&Path>,
    base: Option<&str>,
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
        let inputs = Inputs::capture(&manifest, base)?;
        inputs.verify(&manifest, None)?;
        (Evidence::Source(inputs), manifest)
    };
    // Derive target selection from the bound source rather than trusting an adjacent report
    // that could have been replaced independently of the prepared/preview artifact.
    run_report(output, Some(evidence.base()), &manifest, verbose)?;
    evidence.verify(&manifest)?;
    let report = output.join("report.json");
    let targets = semver_targets(&read_report(&report)?, verbose);
    let cache = cache_path(&env::temp_dir(), evidence.root())?;
    fs::create_dir_all(&cache)?;
    let log = output.join("semver-checks.log");
    let mut log = fs::File::create(log)?;
    let mut outcome = CompatibilityOutcome {
        schema_version: COMPATIBILITY_SCHEMA_VERSION,
        checker: "not invoked: no consumer contracts selected".to_owned(),
        report,
        completed: false,
        findings: false,
        packages: Vec::new(),
    };
    let result = (|| {
        if !targets.is_empty() {
            let checker = invoke(&manifest, &cache, &["--version"])?;
            outcome.identify(&checker)?;
            canary(&cache, &mut log)?;
        }
        let registry = RegistryClient::new(diagnostics.clone())?;
        outcome.assess(
            targets,
            |name| registry.comparison_baseline(name),
            |name, baseline| execute_checker(comparison_command(&manifest, name, baseline), &cache),
            &mut log,
            verbose,
        )
    })();
    let unchanged = evidence.verify(&manifest);
    let destination = output.join("compatibility.json");
    outcome.finish(result, unchanged, |outcome| {
        write_new(&destination, |file| {
            serde_json::to_writer_pretty(file, outcome)
                .map_err(|error| CompatibilityWriteError::caused_by(&destination, error).into())
        })
    })?;
    Ok(outcome.conclusion(output, deny_findings))
}

fn comparison_command(manifest: &Path, name: &str, baseline: &Version) -> Command {
    checker_command(
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

fn invoke(manifest: &Path, cache: &Path, args: &[&str]) -> Result<Output, AppError> {
    execute_checker(checker_command(manifest, args), cache)
}

fn checker_command(manifest: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("cargo");
    command
        .arg("semver-checks")
        .args(args)
        .current_dir(manifest.parent().unwrap_or_else(|| Path::new(".")));
    command
}

fn execute_checker(mut command: Command, cache: &Path) -> Result<Output, AppError> {
    // Windows environment-key ordering calls the OS even before a process starts,
    // so environment configuration belongs to this native execution boundary.
    command
        .env("CARGO_TARGET_DIR", cache)
        .env("CARGO_TERM_COLOR", "never");
    for name in [
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "CARGO_REGISTRY_TOKEN",
        "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
    ] {
        command.env_remove(name);
    }
    command
        .output()
        .map_err(|error| CheckerStartFailed::caused_by(error).into())
}

fn canary(cache: &Path, log: &mut fs::File) -> Result<(), AppError> {
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
    let text = record(&result, log)?;
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

fn record(result: &Output, log: &mut impl Write) -> Result<String, AppError> {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    log.write_all(text.as_bytes())?;
    eprint!("{text}");
    Ok(text)
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

/// Identifies failure to serialize the application's compatibility evidence.
#[ohno::error]
#[display("Failed to write '{}'", path.quoted())]
struct CompatibilityWriteError {
    path: PathBuf,
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
                &mut log,
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
                        &mut log,
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
                    &mut log,
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
                &mut log,
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
                    &mut log,
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
                &mut log,
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
                &mut Vec::new(),
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
                    &mut Vec::new(),
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
                &mut full_log,
                Verbose::new(false, &crp_diag::Discard),
            )
            .unwrap_err();
        assert!(outcome.packages.is_empty());
        assert!(!outcome.findings);
    }

    #[test]
    fn finish_persists_evidence_before_returning_both_failures() {
        for comparison_fails in [false, true] {
            for source_fails in [false, true] {
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
                        Ok(())
                    },
                );
                assert!(persisted.get());
                if comparison_fails || source_fails {
                    let error = result.unwrap_err();
                    assert_eq!(
                        error.find_source::<ComparisonFailure>().is_some(),
                        comparison_fails
                    );
                    if comparison_fails && source_fails {
                        let combined = error.find_source::<ComparisonAndSourceFailed>().unwrap();
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
                        assert_eq!(error.find_source::<SourceFailure>().is_some(), source_fails);
                    }
                } else {
                    result.unwrap();
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

    #[test]
    fn checker_arguments_bind_package_baseline_features_and_source() {
        let manifest = Path::new("candidate").join("Cargo.toml");
        let command = comparison_command(&manifest, "library", &Version::new(1, 2, 3));
        assert_eq!(command.get_program(), "cargo");
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
            checker_command(Path::new(""), &["--version"]).get_current_dir(),
            Some(Path::new("."))
        );
    }

    #[test]
    fn record_keeps_both_streams_readable_when_checker_output_is_not_utf8() {
        let mut log = Vec::new();
        let text = record(&checker_output(1, b"stdout\xff", b"stderr\n"), &mut log).unwrap();
        assert_eq!(text, "stdout\u{fffd}stderr\n");
        assert_eq!(log, text.as_bytes());
    }
}
