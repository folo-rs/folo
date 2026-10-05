use std::fs;

use crate::compatibility::CHECKER_WATCHDOG;
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
            (
                "configuration",
                assessment.fixture.path().join(".cargo/config.toml"),
            ),
            (
                "candidate-configuration",
                candidate(&assessment).join(".cargo/config.toml"),
            ),
        ] {
            let original = fs::read(&path).unwrap();
            let changed = if name.contains("configuration") {
                "[build]\ntarget-dir = 'different-target'\n"
            } else {
                "pub fn drift() {}\n"
            };
            fs::write(&path, changed).unwrap();
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
    ignore = "Changes the index between independent prepared and preview commands"
)]
fn cached_evidence_rejects_index_drift_before_entry_classification() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let assessment = Assessment::new("default");
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
        reused(&success(
            assessment.check("restored-index").output().unwrap(),
        ));
    });
}
