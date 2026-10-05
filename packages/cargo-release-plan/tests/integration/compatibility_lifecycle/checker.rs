use std::fs;

use testing::with_watchdog_timeout;

use crate::compatibility::{CHECKER_WATCHDOG, read_outcome};
use crate::compatibility_lifecycle::{Assessment, same_evidence, success};

#[test]
#[cfg_attr(
    miri,
    ignore = "Traces admitted Cargo/Git workspaces through actual checker processes"
)]
fn compatibility_shares_entry_admission_through_read_only_checker_stages() {
    with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let assessment = Assessment::new();
        for (mode, option, artifact) in [
            ("prepared", "--prepared", "prepared/prepared.json"),
            ("preview", "--plan", "preview/plan.json"),
        ] {
            for state in ["first", "second"] {
                let name = format!("{mode}-{state}");
                let trace = assessment.path(&format!("{name}.trace"));
                let mut command = assessment.check_mode(&name, option, artifact);
                command.env("GIT_TRACE", &trace);
                success(command.output().unwrap());
                // Original admission and separate preview-candidate admission each need a
                // listing; read-only checker stages do not acquire either workspace again.
                let expected = 1 + usize::from(mode == "preview");
                assert_eq!(
                    fs::read_to_string(trace)
                        .unwrap()
                        .lines()
                        .filter(|line| line.ends_with("git ls-files -z -- ':(literal).'"))
                        .count(),
                    expected
                );
                assert_eq!(
                    fs::read_to_string(assessment.path(&format!("{name}.calls"))).unwrap(),
                    "version\ncanary\ncomparison\n"
                );
                if state == "second" {
                    same_evidence(
                        &assessment.path(&format!("{mode}-first")),
                        &assessment.path(&name),
                    );
                }
            }
        }
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Runs genuine checker successes and failures against retained preview evidence"
)]
fn retained_preview_records_comparison_results_and_findings_policy() {
    with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let assessment = Assessment::new();
        for scenario in ["parent-comparison-failure", "parent-compatible"] {
            let result = assessment
                .check(scenario)
                .env("CRP_FIXTURE_SCENARIO", scenario)
                .output()
                .unwrap();
            assert_eq!(result.status.success(), scenario == "parent-compatible");
            let outcome = read_outcome(&assessment.path(scenario));
            assert_eq!(
                outcome.get("completed").unwrap(),
                scenario == "parent-compatible"
            );
            assert_eq!(outcome.get("findings").unwrap(), false);
            let calls = fs::read_to_string(assessment.path(&format!("{scenario}.calls"))).unwrap();
            assert_eq!(calls, "version\ncanary\ncomparison\n");
            assert_eq!(
                assessment
                    .fixture
                    .git(&["worktree", "list", "--porcelain"])
                    .matches("worktree ")
                    .count(),
                1
            );
        }
        let result = assessment
            .check("deny")
            .arg("--deny-findings")
            .output()
            .unwrap();
        assert!(!result.status.success());
        let outcome = read_outcome(&assessment.path("deny"));
        assert_eq!(outcome.get("completed").unwrap(), true);
        assert_eq!(outcome.get("findings").unwrap(), true);
    });
}
