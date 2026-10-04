use std::fs;

use crate::compatibility::{CHECKER_WATCHDOG, configure_git_shim};
use crate::compatibility_cache::{Assessment, candidate, reused, same_evidence, success};

#[test]
#[cfg_attr(
    miri,
    ignore = "Admits an explicit cache override against retained Git evidence"
)]
fn cache_override_cannot_write_into_the_retained_candidate() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let assessment = Assessment::new("default");
        let storage = candidate(&assessment).join("packages/library/src/cache");
        let result = assessment
            .check("overlap")
            .arg("--cache")
            .arg(&storage)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(!storage.exists());
        assert!(!assessment.path("overlap/report.json").exists());
        assert!(!assessment.path("overlap/compatibility.json").exists());
        assert!(!assessment.path("overlap.calls").exists());
        reused(&success(assessment.check("intact").output().unwrap()));
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Changes captured source and live history refs between CLI invocations"
)]
fn cached_preview_rejects_source_candidate_and_named_history_drift_before_report() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let assessment = Assessment::new("absolute");
        reused(&success(assessment.check("warm").output().unwrap()));
        for (name, path) in [
            (
                "source",
                assessment
                    .fixture
                    .path()
                    .join("packages/library/src/lib.rs"),
            ),
            (
                "candidate",
                candidate(&assessment).join("packages/library/src/lib.rs"),
            ),
        ] {
            let original = fs::read(&path).unwrap();
            fs::write(&path, "pub fn drift() {}\n").unwrap();
            rejected_before_report(&assessment, name);
            fs::write(path, original).unwrap();
        }
        for name in ["release-history", "anticipated-parent"] {
            let original = assessment.fixture.sha(name);
            let changed = assessment.fixture.sha("HEAD");
            let reference = format!("refs/heads/{name}");
            assessment
                .fixture
                .git(&["update-ref", &reference, &changed, &original]);
            rejected_before_report(&assessment, name);
            assessment
                .fixture
                .git(&["update-ref", &reference, &original, &changed]);
        }
        reused(&success(assessment.check("restored").output().unwrap()));
        same_evidence(&assessment.path("warm"), &assessment.path("restored"));
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
    assert!(!String::from_utf8_lossy(&result.stderr).contains("reusing classification decisions"));
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Injects source drift immediately after fresh classification hashing"
)]
fn cache_hit_does_not_skip_verification_after_source_acquisition() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let assessment = Assessment::new("default");
        let mut command = assessment.check("drift");
        configure_git_shim(&mut command, assessment.evidence.path());
        let result = command
            .env("CRP_REPORT_DRIFT_AFTER_HASH", "1")
            .env("CRP_REPORT_DRIFT_MARKER", assessment.path("mutated"))
            .output()
            .unwrap();
        reused(&result);
        assert!(!result.status.success());
        assert!(assessment.path("mutated").is_file());
        assert!(assessment.path("drift/report.json").is_file());
        assert!(!assessment.path("drift/compatibility.json").exists());
        assert!(!assessment.path("drift.calls").exists());
    });
}
