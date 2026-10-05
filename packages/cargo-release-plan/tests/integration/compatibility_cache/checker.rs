use std::fs;

use crate::compatibility::{CHECKER_WATCHDOG, read_outcome};
use crate::compatibility_cache::{Assessment, reused, same_evidence, success};

#[test]
#[cfg_attr(
    miri,
    ignore = "Traces admitted Cargo/Git workspaces through actual checker processes"
)]
fn compatibility_shares_entry_admission_through_read_only_checker_stages() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let assessment = Assessment::new("default");
        for (mode, option, artifact) in [
            ("prepared", "--prepared", "prepared/prepared.json"),
            ("preview", "--plan", "preview/plan.json"),
        ] {
            fs::remove_dir_all(&assessment.storage).unwrap();
            for state in ["cold", "warm", "disabled"] {
                let name = format!("{mode}-{state}");
                let trace = assessment.path(&format!("{name}.trace"));
                let mut command = assessment.check_mode(&name, option, artifact);
                command.env("GIT_TRACE", &trace);
                if state == "disabled" {
                    command.arg("--no-cache");
                }
                let output = success(command.output().unwrap());
                if state == "warm" {
                    reused(&output);
                } else if state == "cold" {
                    assert!(
                        String::from_utf8_lossy(&output.stderr)
                            .contains("computed classification decisions")
                    );
                }
                // Original admission, separate preview-candidate admission, and optional
                // storage isolation each need a listing; checker stages need no recapture.
                let expected =
                    1 + usize::from(mode == "preview") + usize::from(state != "disabled");
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
                if state != "cold" {
                    same_evidence(
                        &assessment.path(&format!("{mode}-cold")),
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
    ignore = "Runs genuine checker successes and failures after persisted decision hits"
)]
fn cache_hits_do_not_cache_checker_outcomes() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let assessment = Assessment::new("absolute");
        reused(&success(assessment.check("warm").output().unwrap()));
        for scenario in [
            "identity-failure",
            "canary-failure",
            "parent-comparison-failure",
            "parent-compatible",
        ] {
            let result = assessment
                .check(scenario)
                .env("CRP_FIXTURE_SCENARIO", scenario)
                .output()
                .unwrap();
            reused(&result);
            assert_eq!(result.status.success(), scenario == "parent-compatible");
            let outcome = read_outcome(&assessment.path(scenario));
            assert_eq!(
                outcome.get("completed").unwrap(),
                scenario == "parent-compatible"
            );
            assert_eq!(outcome.get("findings").unwrap(), false);
            let calls = fs::read_to_string(assessment.path(&format!("{scenario}.calls"))).unwrap();
            assert_eq!(
                calls,
                match scenario {
                    "identity-failure" => "version\n",
                    "canary-failure" => "version\ncanary\n",
                    _ => "version\ncanary\ncomparison\n",
                }
            );
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
        reused(&result);
        assert!(!result.status.success());
        assert_eq!(
            read_outcome(&assessment.path("deny"))
                .get("completed")
                .unwrap(),
            true
        );
        let saved = fs::read(assessment.path("deny/compatibility.json")).unwrap();
        assert!(!assessment.check("deny").output().unwrap().status.success());
        assert_eq!(
            fs::read(assessment.path("deny/compatibility.json")).unwrap(),
            saved
        );
    });
}
