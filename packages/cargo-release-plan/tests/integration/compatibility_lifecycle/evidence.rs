use std::fs;

use serde_json::{Value, from_slice};
use testing::with_watchdog_timeout;

use crate::compatibility::{CHECKER_WATCHDOG, checker_command};
use crate::compatibility_lifecycle::{Assessment, same_evidence, success};

#[test]
#[cfg_attr(
    miri,
    ignore = "Uses captured native source with unrelated and absent adjacent reports"
)]
fn captured_evidence_selects_real_targets_independently_of_adjacent_reports() {
    with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let assessment = Assessment::new();
        // This valid report has no targets. Both evidence modes must select the actual
        // captured comparison instead, and remain usable without any adjacent report.
        success(
            checker_command()
                .current_dir(assessment.fixture.path())
                .env_remove("CARGO_TARGET_DIR")
                .args(["report", "--release-history", "HEAD", "--out-dir"])
                .arg(assessment.path("unrelated"))
                .output()
                .unwrap(),
        );
        let unrelated = fs::read(assessment.path("unrelated/report.json")).unwrap();
        let report: Value = from_slice(&unrelated).unwrap();
        assert_eq!(report.pointer("/packages/0/status").unwrap(), "unchanged");
        for (mode, option, artifact) in [
            ("prepared", "--prepared", "prepared/prepared.json"),
            ("preview", "--plan", "preview/plan.json"),
        ] {
            let report = assessment.path(&format!("{mode}/report.json"));
            fs::write(&report, &unrelated).unwrap();
            for state in ["unrelated", "absent"] {
                if state == "absent" {
                    fs::remove_file(&report).unwrap();
                }
                let name = format!("{mode}-{state}");
                success(
                    assessment
                        .check_mode(&name, option, artifact)
                        .output()
                        .unwrap(),
                );
                assert_eq!(
                    fs::read_to_string(assessment.path(&format!("{name}.calls"))).unwrap(),
                    "version\ncanary\ncomparison\n"
                );
            }
            same_evidence(
                &assessment.path(&format!("{mode}-unrelated")),
                &assessment.path(&format!("{mode}-absent")),
            );
        }
    });
}
