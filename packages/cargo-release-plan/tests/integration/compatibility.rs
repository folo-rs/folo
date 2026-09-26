//! Compatibility evidence uses captured inputs without requiring a registry for empty target sets.

use std::fs;

use cargo_release_plan::{RunInput, RunOutcome, run};
use serde_json::Value;
use tempfile::TempDir;

use crate::fixture::{Fixture, write_package};

#[test]
#[cfg_attr(miri, ignore = "Reads real captured source and runs Cargo metadata")]
fn unchanged_workspace_needs_no_checker_or_registry_and_keeps_fresh_report() {
    let fixture = Fixture::new("");
    write_package(&fixture, "library", "1.0.0", "");
    fixture.commit("unchanged source");
    let output = TempDir::new().unwrap();
    let path = output.path().join("evidence");
    let result = run(&RunInput::CheckCompatibility {
        manifest_path: fixture.manifest(),
        prepared: None,
        plan: None,
        base: Some(fixture.sha("HEAD")),
        output: path.clone(),
        deny_findings: true,
        verbose: false,
    })
    .unwrap();
    assert!(matches!(result, RunOutcome::Check { passed: true, .. }));
    let evidence: Value =
        serde_json::from_slice(&fs::read(path.join("compatibility.json")).unwrap()).unwrap();
    assert_eq!(evidence.get("completed").unwrap(), true);
    assert_eq!(evidence.get("findings").unwrap(), false);
    assert!(
        evidence
            .get("packages")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(path.join("report.json").is_file());
}

#[test]
#[cfg_attr(miri, ignore = "Prepares and validates a real Cargo/Git workspace")]
fn prepared_compatibility_rejects_same_head_source_drift_before_comparison() {
    let fixture = Fixture::new("");
    write_package(&fixture, "library", "1.0.0", "");
    fixture.commit("prepared source");
    let output = TempDir::new().unwrap();
    let prepared = output.path().join("prepared");
    run(&RunInput::Prepare {
        output: prepared.clone(),
        base: Some(fixture.sha("HEAD")),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    fixture.write(
        "packages/library/src/lib.rs",
        "pub fn changed_without_commit() {}\n",
    );
    let evidence = output.path().join("compatibility");
    run(&RunInput::CheckCompatibility {
        manifest_path: fixture.manifest(),
        prepared: Some(prepared.join("prepared.json")),
        plan: None,
        base: None,
        output: evidence.clone(),
        deny_findings: false,
        verbose: false,
    })
    .unwrap_err();
    assert!(!evidence.join("compatibility.json").exists());
}
