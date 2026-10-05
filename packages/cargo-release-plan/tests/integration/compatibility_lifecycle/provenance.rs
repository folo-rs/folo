use std::fs;

use crp_versioning::plan::SCHEMA_VERSION;
use serde_json::{Value, json};

use crate::compatibility::{CHECKER_WATCHDOG, checker_command};
use crate::compatibility_lifecycle::{Assessment, same_evidence, success};

#[test]
#[cfg_attr(
    miri,
    ignore = "Retains a second preview and applies actual captured manifest edits"
)]
fn relocated_preview_and_fully_applied_source_preserve_compatibility_evidence() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let assessment = Assessment::new();
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
        for name in ["major-preview", "relocated"] {
            success(
                assessment
                    .command("preview")
                    .arg("--prepared")
                    .arg(assessment.path("prepared/prepared.json"))
                    .arg("--plan")
                    .arg(&proposal)
                    .arg("--output")
                    .arg(assessment.path(name))
                    .output()
                    .unwrap(),
            );
        }
        let check = |name: &str| assessment.check_mode(name, "--plan", "relocated/plan.json");
        success(
            assessment
                .check_mode("initial-location", "--plan", "major-preview/plan.json")
                .output()
                .unwrap(),
        );
        success(check("before").output().unwrap());
        same_evidence(
            &assessment.path("initial-location"),
            &assessment.path("before"),
        );
        success(
            checker_command()
                .current_dir(assessment.fixture.path())
                .args(["apply", "--plan"])
                .env_remove("CARGO_TARGET_DIR")
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
        success(check("after").output().unwrap());
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
