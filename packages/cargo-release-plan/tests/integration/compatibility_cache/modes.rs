use std::fs;

use serde_json::Value;

use crate::compatibility::{CHECKER_WATCHDOG, checker_command, read_outcome};
use crate::compatibility_cache::{Assessment, candidate, reused, same_evidence, success};

#[test]
#[cfg_attr(
    miri,
    ignore = "Uses separate CLI processes and retained Cargo/Git workspaces"
)]
fn prepared_and_preview_checks_use_default_cache() {
    prepared_and_preview_checks("default");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Uses Cargo target-directory environment and a native checker"
)]
fn prepared_and_preview_checks_use_environment_cache() {
    prepared_and_preview_checks("environment");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Uses invocation-relative storage and retained native workspaces"
)]
fn prepared_and_preview_checks_use_relative_cache() {
    prepared_and_preview_checks("relative");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Uses explicit absolute storage and retained native workspaces"
)]
fn prepared_and_preview_checks_use_absolute_cache() {
    prepared_and_preview_checks("absolute");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Executes uncached Git/Cargo acquisition and a native checker"
)]
fn prepared_and_preview_checks_bypass_cache() {
    prepared_and_preview_checks("disabled");
}

fn prepared_and_preview_checks(mode: &'static str) {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, move || {
        let assessment = Assessment::new(mode);
        // A valid report from a different history has no comparison targets. It must not
        // substitute for the source/history bound by either selected evidence artifact.
        success(
            checker_command()
                .current_dir(assessment.fixture.path())
                .args([
                    "report",
                    "--no-cache",
                    "--release-history",
                    "HEAD",
                    "--out-dir",
                ])
                .arg(assessment.path("unrelated"))
                .output()
                .unwrap(),
        );
        let unrelated = fs::read(assessment.path("unrelated/report.json")).unwrap();
        let unrelated_report: Value = serde_json::from_slice(&unrelated).unwrap();
        assert_eq!(
            unrelated_report.pointer("/packages/0/status").unwrap(),
            "unchanged"
        );
        for (name, option, artifact) in [
            ("prepared", "--prepared", "prepared/prepared.json"),
            ("preview", "--plan", "preview/plan.json"),
        ] {
            fs::write(assessment.path(&format!("{name}/report.json")), &unrelated).unwrap();
            let cached_name = format!("{name}-cached");
            let output = success(
                assessment
                    .check_mode(&cached_name, option, artifact)
                    .output()
                    .unwrap(),
            );
            if mode != "disabled" {
                reused(&output);
                assert!(assessment.storage.join("classification-decisions").is_dir());
            } else {
                assert!(
                    !String::from_utf8_lossy(&output.stderr)
                        .contains("reusing classification decisions")
                );
                assert!(!assessment.storage.exists());
            }

            let uncached_name = format!("{name}-uncached");
            let mut uncached = checker_command();
            uncached
                .current_dir(assessment.evidence.path())
                .args(["check-compatibility", "--no-cache", "--manifest-path"])
                .arg(assessment.fixture.manifest())
                .arg(option)
                .arg(assessment.path(artifact))
                .arg("--output")
                .arg(assessment.path(&uncached_name))
                .env("CRP_FIXTURE_SCENARIO", "anticipated-parent")
                .env("CRP_EXPECTED_PARENT", &assessment.parent)
                .env(
                    "CRP_FIXTURE_CALLS",
                    assessment.path(&format!("{uncached_name}.calls")),
                )
                .env(
                    "CRP_FIXTURE_SOURCE",
                    assessment
                        .fixture
                        .path()
                        .join("packages/library/src/lib.rs"),
                );
            success(uncached.output().unwrap());
            same_evidence(
                &assessment.path(&cached_name),
                &assessment.path(&uncached_name),
            );
            for invocation in [&cached_name, &uncached_name] {
                assert_eq!(
                    fs::read_to_string(assessment.path(&format!("{invocation}.calls"))).unwrap(),
                    "version\ncanary\ncomparison\n"
                );
            }
            let outcome = read_outcome(&assessment.path(&cached_name));
            assert_eq!(outcome.get("completed").unwrap(), true);
            assert_eq!(outcome.get("findings").unwrap(), true);
            assert_eq!(
                outcome.pointer("/packages/0/baseline_version").unwrap(),
                "1.1.0"
            );
        }
        assert!(
            !candidate(&assessment)
                .join("configured-target/cargo-release-plan/cache")
                .exists()
        );
        assert_eq!(
            assessment
                .fixture
                .git(&["worktree", "list", "--porcelain"])
                .matches("worktree ")
                .count(),
            1
        );
    });
}
