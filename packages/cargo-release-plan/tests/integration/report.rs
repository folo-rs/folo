//! Report and check output: the JSON document and the failure renderings.

use std::fs;

use cargo_release_plan::{CheckFormat, RunInput, RunOutcome, run};

use crate::fixture::{Fixture, write_package};
use crate::harness::{report_json, seeded_package};

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn github_format_emits_workflow_annotations() {
    let fixture = seeded_package();
    fixture.write("packages/demo/src/lib.rs", "pub fn f() { let _ = 5; }\n");
    fixture.commit("content");
    let base = fixture.sha("HEAD");

    let outcome = run(&RunInput::Check {
        base: Some(base),
        manifest_path: fixture.manifest(),
        format: CheckFormat::Github,
        verify_packaging: false,
        verbose: false,
    })
    .unwrap();
    match outcome {
        RunOutcome::Check { passed, message } => {
            assert!(!passed);
            assert!(message.contains("::error"));
            assert!(message.contains("increment-versions"));
        }
        other => panic!("expected check, got {other:?}"),
    }
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn report_records_group_verdicts() {
    let fixture = Fixture::new(
        r#"
[workspace.metadata.release-plan.groups]
g = ["alpha", "beta"]
"#,
    );
    write_package(&fixture, "alpha", "0.1.0", "");
    write_package(&fixture, "beta", "0.1.0", "");
    fixture.commit("seed");
    let base = fixture.sha("HEAD");

    let report = report_json(&fixture, &base);

    assert!(report.contains("\"consistent\": true"), "{report}");
    assert!(report.contains("\"alpha\""), "{report}");
    assert!(report.contains("\"beta\""), "{report}");
    assert!(report.contains("\"version\": \"0.1.0\""), "{report}");
}

/// Report replaces the diffs of an earlier run.
///
/// A report directory is reused across runs, so a diff left over from a package that no longer has
/// one would still be read as current.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn report_replaces_the_diffs_of_an_earlier_run() {
    let fixture = Fixture::new("");
    write_package(&fixture, "demo", "0.1.0", "");
    fixture.commit("seed");
    let base = fixture.sha("HEAD");
    let out_dir = fixture.path().join("out");
    report_json(&fixture, &base);
    let stale = out_dir.join("diffs").join("stale.diff");
    fs::write(&stale, "leftover").unwrap();

    report_json(&fixture, &base);

    assert!(!stale.exists());
    assert!(out_dir.join("report.json").exists());
}
