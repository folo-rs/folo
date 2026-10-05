use std::fs;

use testing::with_watchdog_timeout;

use crate::compatibility::CHECKER_WATCHDOG;
use crate::compatibility_lifecycle::{Assessment, candidate, same_evidence, success};

#[test]
#[cfg_attr(
    miri,
    ignore = "Changes captured configuration and live history refs between CLI invocations"
)]
fn preview_rejects_configuration_and_named_history_drift_before_report() {
    with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let assessment = Assessment::new();
        success(assessment.check("original").output().unwrap());
        let path = candidate(&assessment).join(".cargo/config.toml");
        let original = fs::read(&path).unwrap();
        fs::write(&path, "[build]\ntarget-dir = 'different-target'\n").unwrap();
        rejected_before_report(&assessment, "candidate-configuration");
        fs::write(path, original).unwrap();
        let original = assessment.fixture.sha("release-history");
        let changed = assessment.fixture.sha("HEAD");
        let reference = "refs/heads/release-history";
        assessment
            .fixture
            .git(&["update-ref", reference, &changed, &original]);
        rejected_before_report(&assessment, "release-history");
        assessment
            .fixture
            .git(&["update-ref", reference, &original, &changed]);
        success(assessment.check("restored").output().unwrap());
        same_evidence(&assessment.path("original"), &assessment.path("restored"));
    });
}

fn rejected_before_report(assessment: &Assessment, name: &str) {
    let result = assessment.check(name).output().unwrap();
    assert!(!result.status.success());
    assert!(!assessment.path(&format!("{name}/report.json")).exists());
    assert!(
        !assessment
            .path(&format!("{name}/compatibility.json"))
            .exists()
    );
    assert!(!assessment.path(&format!("{name}.calls")).exists());
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Changes the index between independent prepared and preview commands"
)]
fn captured_evidence_rejects_index_drift_before_entry_classification() {
    with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let assessment = Assessment::new();
        let path = "packages/library/src/lib.rs";
        let original = assessment.fixture.read(path);
        assessment.fixture.write(path, "pub fn staged() {}\n");
        assessment.fixture.git(&["add", "--", path]);
        assessment.fixture.write(path, &original);
        rejected_before_report(&assessment, "index");
        let result = assessment
            .check_mode("prepared-index", "--prepared", "prepared/prepared.json")
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(!assessment.path("prepared-index/report.json").exists());
        assert!(!assessment.path("prepared-index.calls").exists());
        assessment.fixture.git(&["add", "--", path]);
        success(assessment.check("restored-index").output().unwrap());
    });
}
