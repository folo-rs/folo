use std::fs;

use crate::compatibility::{CHECKER_WATCHDOG, read_outcome};
use crate::compatibility_cache::{Assessment, candidate, reused, success};

#[test]
#[cfg_attr(
    miri,
    ignore = "Runs cached preview admission and a native checker mutation"
)]
fn preview_cache_hit_rechecks_source_and_candidate_around_checker() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let assessment = Assessment::new("default");
        for (subject, source) in [
            (
                "original",
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
            let original = fs::read(&source).unwrap();
            for phase in ["canary", "comparison"] {
                let name = format!("{subject}-{phase}");
                let output = assessment
                    .check(&name)
                    .env("CRP_FIXTURE_MUTATION_PHASE", phase)
                    .env("CRP_FIXTURE_MUTATION_PATH", &source)
                    .output()
                    .unwrap();
                reused(&output);
                assert!(!output.status.success());
                // Canary drift must stop before baseline assessment, not just checker execution.
                assert_eq!(
                    String::from_utf8_lossy(&output.stderr)
                        .contains("Comparing library against baseline"),
                    phase == "comparison"
                );
                let outcome = read_outcome(&assessment.path(&name));
                assert_eq!(outcome.get("completed").unwrap(), false);
                assert_eq!(outcome.get("findings").unwrap(), phase == "comparison");
                assert_eq!(
                    fs::read_to_string(assessment.path(&format!("{name}.calls"))).unwrap(),
                    if phase == "canary" {
                        "version\ncanary\n"
                    } else {
                        "version\ncanary\ncomparison\n"
                    }
                );
                assert!(
                    fs::read_to_string(&source)
                        .unwrap()
                        .contains("changed_during")
                );
                fs::write(&source, &original).unwrap();
            }
        }
        assert!(candidate(&assessment).join("Cargo.toml").is_file());
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Runs checker failures and parent mutations after persisted cache hits"
)]
fn cache_hits_do_not_cache_checker_success_or_parent_verification() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let assessment = Assessment::new("absolute");
        reused(&success(assessment.check("warm").output().unwrap()));
        let history = assessment.fixture.sha("release-history");
        for scenario in [
            "identity-failure",
            "canary-failure",
            "parent-comparison-failure",
            "parent-source-drift",
            "parent-head-drift",
            "parent-target-drift",
        ] {
            let result = assessment
                .check(scenario)
                .env("CRP_FIXTURE_SCENARIO", scenario)
                .env("CRP_FIXTURE_HISTORY", &history)
                .env("CRP_FIXTURE_ROOT", assessment.fixture.path())
                .output()
                .unwrap();
            reused(&result);
            assert!(!result.status.success());
            let outcome = read_outcome(&assessment.path(scenario));
            assert_eq!(outcome.get("completed").unwrap(), false);
            let calls = fs::read_to_string(assessment.path(&format!("{scenario}.calls"))).unwrap();
            assert_eq!(
                calls,
                match scenario {
                    "identity-failure" => "version\n",
                    "canary-failure" | "parent-target-drift" => "version\ncanary\n",
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
            if scenario == "parent-target-drift" {
                assessment.fixture.git(&[
                    "update-ref",
                    "refs/heads/anticipated-parent",
                    &assessment.parent,
                    &history,
                ]);
            }
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
