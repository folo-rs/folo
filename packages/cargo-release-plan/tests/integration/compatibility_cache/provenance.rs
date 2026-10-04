use std::fs;
use std::path::Path;

use crp_versioning::plan::SCHEMA_VERSION;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::compatibility::{CHECKER_WATCHDOG, checker_command};
use crate::compatibility_cache::{Assessment, reused, same_evidence, success};
use crate::fixture::{Fixture, write_package};

#[test]
#[cfg_attr(
    miri,
    ignore = "Shares decisions between distinct real Git/Cargo repositories"
)]
fn shared_decisions_keep_current_provenance_and_changed_inputs_recompute() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let fixture = Fixture::new("");
        write_package(
            &fixture,
            "library",
            "1.0.0",
            "[package.metadata.release-plan]\nprivate-api = true\n",
        );
        fixture.commit("private library history");
        let history = fixture.sha("HEAD");
        fixture.write("packages/library/src/lib.rs", "pub fn changed() {}\n");
        let evidence = TempDir::new().unwrap();
        let storage = evidence.path().join("cache");
        let prepared = evidence.path().join("prepared");
        success(
            checker_command()
                .current_dir(fixture.path())
                .args(["prepare", "--release-history", &history, "--cache"])
                .arg(&storage)
                .arg("--output")
                .arg(&prepared)
                .output()
                .unwrap(),
        );
        let other = Fixture::from_template(&fixture);
        other.git(&["commit", "--allow-empty", "-m", "new current provenance"]);
        let fresh = |output: &Path| {
            let mut command = checker_command();
            command
                .current_dir(other.path())
                .args([
                    "check-compatibility",
                    "--verbose",
                    "--release-history",
                    &history,
                    "--output",
                ])
                .arg(output);
            command
        };
        let warm = evidence.path().join("warm");
        reused(&success(
            fresh(&warm).arg("--cache").arg(&storage).output().unwrap(),
        ));
        let mut expected: Value =
            serde_json::from_slice(&fs::read(prepared.join("report.json")).unwrap()).unwrap();
        let current: Value =
            serde_json::from_slice(&fs::read(warm.join("report.json")).unwrap()).unwrap();
        assert_ne!(expected.get("head").unwrap(), current.get("head").unwrap());
        assert_eq!(current.get("head").unwrap(), &other.sha("HEAD"));
        *expected.get_mut("head").unwrap() = current.get("head").unwrap().clone();
        assert_eq!(current, expected);

        let wrong = evidence.path().join("wrong-source");
        let rejected = checker_command()
            .current_dir(other.path())
            .args(["check-compatibility", "--verbose", "--prepared"])
            .arg(prepared.join("prepared.json"))
            .arg("--cache")
            .arg(&storage)
            .arg("--output")
            .arg(&wrong)
            .output()
            .unwrap();
        assert!(!rejected.status.success());
        assert!(!wrong.join("report.json").exists());
        assert!(
            !String::from_utf8_lossy(&rejected.stderr).contains("reusing classification decisions")
        );

        other.write(
            "packages/library/src/lib.rs",
            "pub fn different_input() {}\n",
        );
        let changed = evidence.path().join("changed");
        let result = success(
            fresh(&changed)
                .arg("--cache")
                .arg(&storage)
                .output()
                .unwrap(),
        );
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("computed classification decisions")
        );
        assert!(
            !String::from_utf8_lossy(&result.stderr).contains("reusing classification decisions")
        );
        let uncached = evidence.path().join("uncached");
        success(fresh(&uncached).arg("--no-cache").output().unwrap());
        same_evidence(&changed, &uncached);
        assert_ne!(
            fs::read(warm.join("diffs/library.patch")).unwrap(),
            fs::read(changed.join("diffs/library.patch")).unwrap()
        );
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Retains a second preview and applies actual captured manifest edits"
)]
fn relocated_preview_and_fully_applied_source_keep_original_cache_ownership() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let assessment = Assessment::new("relative");
        let proposal = assessment.path("major.json");
        fs::write(
            &proposal,
            serde_json::to_vec(&json!({
                "schema_version": SCHEMA_VERSION,
                "increments": [{"name": "library", "bump": "major"}]
            }))
            .unwrap(),
        )
        .unwrap();
        let preview = |name: &str| {
            let mut command = assessment.command("preview");
            command
                .arg("--prepared")
                .arg(assessment.path("prepared/prepared.json"))
                .arg("--plan")
                .arg(&proposal)
                .arg("--output")
                .arg(assessment.path(name));
            command
        };
        success(preview("major-preview").output().unwrap());
        reused(&success(preview("relocated").output().unwrap()));
        let check = |name: &str| assessment.check_mode(name, "--plan", "relocated/plan.json");
        reused(&success(check("before").output().unwrap()));
        success(
            checker_command()
                .current_dir(assessment.fixture.path())
                .args(["apply", "--plan"])
                .arg(assessment.path("relocated/plan.json"))
                .output()
                .unwrap(),
        );
        assert!(
            assessment
                .fixture
                .read("packages/library/Cargo.toml")
                .contains("2.0.0")
        );
        reused(&success(check("after").output().unwrap()));
        same_evidence(&assessment.path("before"), &assessment.path("after"));
        assert_eq!(
            serde_json::from_slice::<Value>(
                &fs::read(assessment.path("after/report.json")).unwrap()
            )
            .unwrap()
            .pointer("/packages/0/declared_version")
            .unwrap(),
            "2.0.0"
        );
    });
}
