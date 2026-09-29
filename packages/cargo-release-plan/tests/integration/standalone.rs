//! One-off version planning through the executable, without publishing integration.

#![cfg_attr(coverage_nightly, coverage(off))]

use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::fixture::{Fixture, write_package};

#[test]
#[cfg_attr(
    miri,
    ignore = "Runs the versioning CLI, Git and Cargo in a local repository"
)]
fn standalone_versions_reach_the_merged_tree_without_publishing_setup() {
    // Repeated Git/Cargo startup is ordinarily seconds of work. Keep the last-chance
    // timeout generous for instrumented Windows runs; it is not a failure assertion.
    testing::with_watchdog_timeout(Duration::from_mins(5), || {
        let fixture = Fixture::new("");
        write_package(
            &fixture,
            "support",
            "1.0.0",
            "[package.metadata.release-plan]\nprivate-api = true\n",
        );
        write_package(
            &fixture,
            "utility",
            "1.0.0",
            "[dependencies]\nsupport = { path = '../support', version = '=1.0.0' }\n",
        );
        fs::remove_file(fixture.path().join("packages/utility/src/lib.rs")).unwrap();
        fixture.write("packages/utility/src/main.rs", "fn main() {}\n");
        fixture.cargo(&["generate-lockfile", "--offline"]);
        fixture.commit("manual release history");
        let history = fixture.sha("HEAD");
        fixture.git(&["switch", "-c", "one-off"]);
        fixture.write("packages/support/src/lib.rs", "pub fn changed() {}\n");
        assert!(fixture.git(&["remote"]).trim().is_empty());
        assert!(!fixture.path().join(".cargo/release_plan.toml").exists());
        assert!(!fixture.path().join(".github").exists());
        let before = command(&fixture)
            .args(["check", "--release-history", &history])
            .output()
            .unwrap();
        assert!(!before.status.success());

        let artifacts = TempDir::new().unwrap();
        let prepared = artifacts.path().join("prepared");
        success(
            command(&fixture)
                .args(["prepare", "--release-history", &history, "--output"])
                .arg(&prepared),
        );
        let compatibility = artifacts.path().join("compatibility");
        success(
            command(&fixture)
                .args(["check-compatibility", "--prepared"])
                .arg(prepared.join("prepared.json"))
                .arg("--output")
                .arg(&compatibility),
        );
        complete_comparison(&compatibility);
        let report = prepared.join("report.json");
        success(
            command(&fixture)
                .args(["analysis-order", "--report"])
                .arg(&report),
        );
        let decisions = artifacts.path().join("decisions.json");
        fs::write(
            &decisions,
            serde_json::to_vec(&json!({
                "schema_version": 2,
                "changes": [{"name": "support", "impact": "patch"}]
            }))
            .unwrap(),
        )
        .unwrap();
        let proposal = artifacts.path().join("proposal.json");
        success(
            command(&fixture)
                .args(["propose", "--report"])
                .arg(&report)
                .arg("--decisions")
                .arg(decisions)
                .arg("--out")
                .arg(&proposal),
        );
        let preview = artifacts.path().join("preview");
        success(
            command(&fixture)
                .args(["preview", "--prepared"])
                .arg(prepared.join("prepared.json"))
                .arg("--plan")
                .arg(proposal)
                .arg("--output")
                .arg(&preview),
        );
        let plan = preview.join("plan.json");
        let preview_comparison = artifacts.path().join("preview-compatibility");
        success(
            command(&fixture)
                .args(["check-compatibility", "--plan"])
                .arg(&plan)
                .arg("--output")
                .arg(&preview_comparison),
        );
        complete_comparison(&preview_comparison);

        // A version edit does not require registry identities or a publishing configuration.
        success(command(&fixture).args(["apply", "--plan"]).arg(plan));
        fixture.cargo(&["metadata", "--locked", "--offline", "--format-version", "1"]);
        let verified = artifacts.path().join("verified");
        success(
            command(&fixture)
                .args([
                    "check-compatibility",
                    "--release-history",
                    &history,
                    "--output",
                ])
                .arg(&verified),
        );
        complete_comparison(&verified);
        success(command(&fixture).args(["check", "--release-history", &history]));
        let report: Value =
            serde_json::from_slice(&fs::read(verified.join("report.json")).unwrap()).unwrap();
        let packages = report.get("packages").unwrap().as_array().unwrap();
        assert_eq!(packages.len(), 2);
        for package in packages {
            assert_eq!(package.get("declared_version").unwrap(), "1.0.1");
        }
        let preview_plan: Value =
            serde_json::from_slice(&fs::read(preview.join("plan.json")).unwrap()).unwrap();
        for file in preview_plan
            .pointer("/resolved/files")
            .unwrap()
            .as_array()
            .unwrap()
        {
            let path = file.get("path").unwrap().as_str().unwrap();
            assert_eq!(
                fixture.read(path),
                file.get("contents").unwrap().as_str().unwrap()
            );
        }

        // Model the authorized PR merge locally; no publishing operation follows it.
        fixture.commit("reviewed one-off increments");
        fixture.git(&["switch", "main"]);
        fixture.git(&["merge", "--squash", "one-off"]);
        fixture.commit("merge version-increment PR");
        success(command(&fixture).args(["check", "--release-history", "HEAD"]));
        assert!(fixture.git(&["status", "--porcelain"]).trim().is_empty());
        assert!(!fixture.path().join(".cargo/release_plan.toml").exists());
        assert!(!fixture.path().join(".github").exists());
        assert!(fixture.git(&["remote"]).trim().is_empty());
    });
}

fn command(fixture: &Fixture) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-release-plan"));
    command
        .current_dir(fixture.path())
        .env("CARGO_NET_OFFLINE", "true");
    command
}

fn success(command: &mut Command) -> Output {
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{:?}\n{}",
        command.get_args().collect::<Vec<_>>(),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn complete_comparison(directory: &Path) {
    let result: Value =
        serde_json::from_slice(&fs::read(directory.join("compatibility.json")).unwrap()).unwrap();
    assert_eq!(result.get("completed").unwrap(), true);
    assert_eq!(result.get("findings").unwrap(), false);
    // These packages have no public Rust library contract; this run needs no registry baseline.
    assert!(
        result
            .get("packages")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
}
