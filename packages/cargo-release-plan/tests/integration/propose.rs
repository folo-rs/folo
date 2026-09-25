//! Proposal artifact acquisition, input protection and output invalidation.

use std::fs;
use std::path::Path;

use cargo_release_plan::{RunInput, RunOutcome, run};
use ohno::AppError;
use serde_json::{Value, json};
use tempfile::tempdir;

#[test]
#[cfg_attr(miri, ignore = "resolves real proposal input and output paths")]
fn missing_output_parents_cannot_make_an_input_writable_as_a_proposal() {
    let directory = tempdir().unwrap();
    let report_path = directory.path().join("report.json");
    let decisions_path = directory.path().join("decisions.json");
    fs::write(&report_path, report().to_string()).unwrap();
    fs::write(&decisions_path, r#"{"schema_version":1,"changes":[]}"#).unwrap();
    for input in [&report_path, &decisions_path] {
        let before = fs::read(input).unwrap();
        let output = directory
            .path()
            .join("missing")
            .join("..")
            .join(input.file_name().unwrap());
        propose(&report_path, &decisions_path, &output).unwrap_err();
        assert_eq!(fs::read(input).unwrap(), before);
        assert!(!directory.path().join("missing").exists());
    }
}

#[test]
#[cfg_attr(miri, ignore = "validates real proposal input and output paths")]
fn unusable_output_path_is_rejected_without_damaging_inputs() {
    let directory = tempdir().unwrap();
    let report_path = directory.path().join("report.json");
    let decisions_path = directory.path().join("decisions.json");
    // An interior NUL rejects the path before publication without permission assumptions.
    let output = directory.path().join("invalid\0.json");
    fs::write(&report_path, report().to_string()).unwrap();
    fs::write(&decisions_path, r#"{"schema_version":1,"changes":[]}"#).unwrap();
    let valid_output = directory.path().join("proposal.json");
    propose(&report_path, &decisions_path, &valid_output).unwrap();
    assert!(valid_output.is_file());
    let original_report = fs::read(&report_path).unwrap();
    let original_decisions = fs::read(&decisions_path).unwrap();
    propose(&report_path, &decisions_path, &output).unwrap_err();
    assert!(!output.exists());
    assert_eq!(fs::read(&report_path).unwrap(), original_report);
    assert_eq!(fs::read(&decisions_path).unwrap(), original_decisions);
}

#[test]
#[cfg_attr(miri, ignore = "reads and writes real proposal artifacts")]
fn failed_reruns_remove_stale_proposals_and_preserve_inputs() {
    let directory = tempdir().unwrap();
    let report_path = directory.path().join("report.json");
    let decisions_path = directory.path().join("decisions.json");
    let output = directory.path().join("output/plan.json");
    fs::write(&report_path, report().to_string()).unwrap();
    fs::write(
        &decisions_path,
        r#"{"schema_version":1,"changes":[{"name":"library","level":"patch"}]}"#,
    )
    .unwrap();
    propose(directory.path(), &decisions_path, &output).unwrap();
    let written: Value = serde_json::from_slice(&fs::read(&output).unwrap()).unwrap();
    assert_eq!(
        written.get("increments").unwrap(),
        &json!([{"name": "library", "level": "patch"}])
    );
    fs::write(&decisions_path, "null").unwrap();
    propose(&report_path, &decisions_path, &output).unwrap_err();
    assert!(!output.exists());
    let before = fs::read(&report_path).unwrap();
    propose(directory.path(), &decisions_path, &report_path).unwrap_err();
    assert_eq!(fs::read(&report_path).unwrap(), before);
    propose(&report_path, &decisions_path, &decisions_path).unwrap_err();
    assert_eq!(fs::read_to_string(&decisions_path).unwrap(), "null");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "reads malformed report artifacts and invalidates stale output"
)]
fn malformed_versions_anywhere_in_the_report_invalidate_stale_output() {
    let directory = tempdir().unwrap();
    let report_path = directory.path().join("report.json");
    let decisions_path = directory.path().join("decisions.json");
    let output = directory.path().join("plan.json");
    fs::write(&decisions_path, r#"{"schema_version":1,"changes":[]}"#).unwrap();
    let mut report = report();
    *report.get_mut("non_publishable_packages").unwrap() = json!([
        {"name": "helper", "declared_version": "1.0.0", "group": "helper"},
        {"name": "support", "declared_version": "1.1.0", "group": "helper"}
    ]);
    *report.get_mut("groups").unwrap() = json!({
        "helper": {"members": ["helper", "support"], "consistent": false, "version": "1.1.0"}
    });
    // Establish a valid control before independently corrupting each version source.
    fs::write(&report_path, report.to_string()).unwrap();
    propose(&report_path, &decisions_path, &output).unwrap();
    for pointer in [
        "/packages/0/declared_version",
        "/packages/0/anchor/version",
        "/non_publishable_packages/0/declared_version",
        "/groups/helper/version",
    ] {
        let mut value = report.clone();
        *value.pointer_mut(pointer).unwrap() = json!("invalid");
        fs::write(&report_path, value.to_string()).unwrap();
        fs::write(&output, "stale").unwrap();
        propose(&report_path, &decisions_path, &output).unwrap_err();
        assert!(!output.exists());
    }
}

fn propose(report: &Path, decisions: &Path, output: &Path) -> Result<RunOutcome, AppError> {
    run(&RunInput::Propose {
        report: report.to_path_buf(),
        decisions: decisions.to_path_buf(),
        out: output.to_path_buf(),
        verbose: false,
    })
}

fn report() -> Value {
    json!({
        "schema_version": 4,
        "head": "captured",
        "packages": [{
            "name": "library",
            "declared_version": "1.0.0",
            "status": "unchanged",
            "anchor": {"commit": "anchor", "version": "1.0.0"},
            "changed": [],
            "stat": {"files": 0, "insertions": 0, "deletions": 0},
            "dependencies": [],
            "dependents": [],
            "consumer_contract": true
        }],
        "non_publishable_packages": [],
        "groups": {}
    })
}
