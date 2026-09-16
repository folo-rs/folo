#![cfg_attr(coverage_nightly, coverage(off))]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use cargo_release_plan::{CheckFormat, RunInput, RunOutcome};
use ohno::AppError;
use semver::Version;
use serde_json::json;

use crate::Metadata;
use crate::cli::Cli;
use crate::repository::VerificationError;
use crate::verification_repository::VerificationRepository;
use crate::verify::verify_using;

// Each external operation is bracketed by clean-head checks, even when it fails.
const SEQUENCE: &[&str] = &[
    "discover",
    "clean",
    "first-parent",
    "tracked",
    "clean",
    "metadata",
    "clean",
    "inputs",
    "clean",
    "checker",
    "clean",
];

/// Supplies in-memory evidence and records the sequence without running Git, Cargo or filesystem I/O.
struct Evidence<'a> {
    cli: &'a Cli,
    calls: &'a RefCell<Vec<&'static str>>,
    failures: &'a [usize],
    metadata: &'a [u8],
    outcome: RunOutcome,
}

impl Evidence<'_> {
    fn step(&self, name: &'static str) -> Result<(), AppError> {
        let mut calls = self.calls.borrow_mut();
        let step = calls.len();
        calls.push(name);
        if self.failures.contains(&step) {
            return Err(BoundaryError::new(step).into());
        }
        Ok(())
    }

    fn verify(self, diagnostic: impl FnMut(&str)) -> Result<String, AppError> {
        verify_using(
            self.cli,
            |manifest, commit| {
                assert_eq!(manifest, self.cli.manifest_path);
                assert_eq!(commit, self.cli.commit);
                self.step("discover")?;
                Ok(self)
            },
            diagnostic,
        )
    }
}

impl VerificationRepository for Evidence<'_> {
    fn ensure_clean_head(&self) -> Result<(), AppError> {
        self.step("clean")
    }

    fn ensure_first_parent(&self, release_line: &str) -> Result<(), AppError> {
        assert_eq!(release_line, self.cli.release_line);
        self.step("first-parent")
    }

    fn require_tracked(&self, manifest: &Path) -> Result<PathBuf, AppError> {
        assert_eq!(manifest, self.cli.manifest_path);
        self.step("tracked")?;
        Ok(canonical_manifest())
    }

    fn capture(&self, program: &str, arguments: &[&OsStr]) -> Result<Vec<u8>, AppError> {
        assert_eq!(program, "cargo");
        let manifest = canonical_manifest();
        assert_eq!(
            arguments,
            [
                OsStr::new("metadata"),
                OsStr::new("--format-version"),
                OsStr::new("1"),
                OsStr::new("--locked"),
                OsStr::new("--offline"),
                OsStr::new("--no-deps"),
                OsStr::new("--manifest-path"),
                manifest.as_os_str(),
            ]
        );
        self.step("metadata")?;
        Ok(self.metadata.to_vec())
    }

    fn validate_inputs(&self, _metadata: &Metadata, manifest: &Path) -> Result<(), AppError> {
        assert_eq!(manifest, canonical_manifest());
        self.step("inputs")
    }

    fn check(&self, input: &RunInput) -> Result<RunOutcome, AppError> {
        let RunInput::Check {
            base,
            manifest_path,
            format,
            verify_packaging,
            verbose,
        } = input
        else {
            panic!();
        };
        assert_eq!(base.as_ref(), Some(&self.cli.commit));
        assert_eq!(*manifest_path, canonical_manifest());
        assert!(matches!(format, CheckFormat::Text));
        assert!(!verify_packaging);
        assert_eq!(*verbose, self.cli.verbose);
        self.step("checker")?;
        Ok(self.outcome.clone())
    }
}

/// Identifies which injected boundary failure survives orchestration and its recheck.
#[ohno::error]
struct BoundaryError {
    step: usize,
}

fn cli(verbose: bool) -> Cli {
    Cli {
        manifest_path: Path::new("requested").join("Cargo.toml"),
        // Distinct opaque IDs detect mixing candidate and release-line responsibilities.
        commit: "candidate".into(),
        release_line: "later-main".into(),
        packages: BTreeMap::from([
            ("widget".into(), Version::new(1, 2, 3)),
            ("alpha".into(), Version::new(2, 0, 0)),
        ]),
        verbose,
    }
}

fn canonical_manifest() -> PathBuf {
    Path::new("canonical").join("Cargo.toml")
}

fn metadata() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "workspace_root": "canonical",
        "workspace_members": ["widget-id", "alpha-id"],
        "packages": [
            {"name": "widget", "id": "widget-id", "version": "1.2.3",
             "manifest_path": canonical_manifest(), "publish": null},
            {"name": "alpha", "id": "alpha-id", "version": "2.0.0",
             "manifest_path": canonical_manifest(), "publish": null}
        ]
    }))
    .unwrap()
}

fn passed() -> RunOutcome {
    RunOutcome::Check {
        passed: true,
        message: "checker verdict canary".into(),
        warnings: "checker warning canary".into(),
    }
}

#[test]
fn verifies_the_complete_sequence_before_emitting_success() {
    for verbose in [false, true] {
        let cli = cli(verbose);
        let calls = RefCell::new(Vec::new());
        let mut diagnostics = Vec::new();
        let message = Evidence {
            cli: &cli,
            calls: &calls,
            failures: &[],
            metadata: &metadata(),
            outcome: passed(),
        }
        .verify(|message| diagnostics.push(message.to_owned()))
        .unwrap();
        assert_eq!(*calls.borrow(), SEQUENCE);
        assert_eq!(
            message,
            "Verified release target candidate: alpha@2.0.0, widget@1.2.3."
        );
        if verbose {
            let [identity, baseline, warning, verdict] = diagnostics.as_slice() else {
                panic!();
            };
            assert!(identity.contains("candidate"));
            assert!(identity.contains("later-main"));
            assert!(baseline.contains("candidate"));
            assert!(baseline.contains("later-main"));
            assert_eq!(warning, "checker warning canary");
            assert!(verdict.contains("checker verdict canary"));
        } else {
            assert_eq!(diagnostics, ["checker warning canary"]);
        }
    }
}

#[test]
fn each_boundary_failure_stops_verification_after_required_rechecks() {
    let cli = cli(false);
    let metadata = metadata();
    for (step, operation) in SEQUENCE.iter().enumerate() {
        let calls = RefCell::new(Vec::new());
        let error = Evidence {
            cli: &cli,
            calls: &calls,
            failures: &[step],
            metadata: &metadata,
            outcome: passed(),
        }
        .verify(|_| panic!())
        .unwrap_err();
        assert_eq!(error.find_source::<BoundaryError>().unwrap().step, step);
        let end = step + 1 + usize::from(matches!(*operation, "metadata" | "checker"));
        assert_eq!(*calls.borrow(), SEQUENCE.get(..end).unwrap());
    }
}

#[test]
fn failed_operations_cannot_hide_a_failed_repository_recheck() {
    let cli = cli(false);
    let metadata = metadata();
    for operation in ["metadata", "checker"] {
        let step = SEQUENCE.iter().position(|item| *item == operation).unwrap();
        let calls = RefCell::new(Vec::new());
        let error = Evidence {
            cli: &cli,
            calls: &calls,
            failures: &[step, step + 1],
            metadata: &metadata,
            outcome: passed(),
        }
        .verify(|_| panic!())
        .unwrap_err();
        assert_eq!(error.find_source::<BoundaryError>().unwrap().step, step + 1);
        assert_eq!(*calls.borrow(), SEQUENCE.get(..=step + 1).unwrap());
    }
}

#[test]
fn malformed_metadata_is_rejected_before_input_validation() {
    let cli = cli(false);
    let calls = RefCell::new(Vec::new());
    let error = Evidence {
        cli: &cli,
        calls: &calls,
        failures: &[],
        metadata: b"not JSON",
        outcome: passed(),
    }
    .verify(|_| panic!())
    .unwrap_err();
    assert!(error.find_source::<VerificationError>().is_some());
    let inputs = SEQUENCE.iter().position(|item| *item == "inputs").unwrap();
    assert_eq!(*calls.borrow(), SEQUENCE.get(..inputs).unwrap());
}

#[test]
fn package_identity_must_match_before_running_the_checker() {
    let mut cli = cli(false);
    cli.packages.insert("widget".into(), Version::new(9, 0, 0));
    let calls = RefCell::new(Vec::new());
    let error = Evidence {
        cli: &cli,
        calls: &calls,
        failures: &[],
        metadata: &metadata(),
        outcome: passed(),
    }
    .verify(|_| panic!())
    .unwrap_err();
    assert!(error.find_source::<VerificationError>().is_some());
    let inputs = SEQUENCE.iter().position(|item| *item == "inputs").unwrap();
    assert_eq!(*calls.borrow(), SEQUENCE.get(..=inputs).unwrap());
}

#[test]
fn only_a_passing_check_outcome_certifies_the_candidate() {
    let cli = cli(false);
    let metadata = metadata();
    for outcome in [
        RunOutcome::Check {
            passed: false,
            message: "rejection canary".into(),
            warnings: "warning canary".into(),
        },
        RunOutcome::Report {
            message: "not a check".into(),
        },
    ] {
        let expected = if matches!(outcome, RunOutcome::Check { .. }) {
            vec!["warning canary"]
        } else {
            vec![]
        };
        let calls = RefCell::new(Vec::new());
        let mut diagnostics = Vec::new();
        let error = Evidence {
            cli: &cli,
            calls: &calls,
            failures: &[],
            metadata: &metadata,
            outcome,
        }
        .verify(|message| diagnostics.push(message.to_owned()))
        .unwrap_err();
        assert!(error.find_source::<VerificationError>().is_some());
        assert_eq!(*calls.borrow(), SEQUENCE);
        assert_eq!(diagnostics, expected);
    }
}
