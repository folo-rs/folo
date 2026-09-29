//! Captured-source binding and checker failures without production registry access.
//!
//! Fixture versions are ordinary valid releases; exact dependency fixtures retain matching
//! declarations to exercise group propagation, while `private_library` supports patch planning.

#![cfg_attr(coverage_nightly, coverage(off))]

use std::env::consts::EXE_SUFFIX;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
use std::time::Duration;
use std::{env, fs, iter};

use cargo_release_plan::{RunInput, RunOutcome, run};
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::fixture::{Fixture, write_package};
use crate::harness::resolved_plan;

// Git/Cargo startup and checker-fixture compilation normally finish in seconds. This deliberately
// conservative budget protects infrastructure hangs, never determines an expected failure.
const CHECKER_WATCHDOG: Duration = Duration::from_mins(5);

#[test]
#[cfg_attr(miri, ignore = "Reads real captured source and runs Cargo metadata")]
fn unchanged_workspace_needs_no_checker_or_registry_and_keeps_fresh_report() {
    let fixture = Fixture::new("");
    // The version is representative: this case exercises empty selection, not semver boundaries.
    write_package(&fixture, "library", "1.0.0", "");
    fixture.commit("unchanged source");
    let output = TempDir::new().unwrap();
    let path = output.path().join("evidence");
    let result = run(&RunInput::CheckCompatibility {
        manifest_path: fixture.manifest(),
        prepared: None,
        plan: None,
        release_history: Some(fixture.sha("HEAD")),
        merge_target: None,
        output: path.clone(),
        deny_findings: true,
        verbose: false,
    })
    .unwrap();
    assert!(matches!(result, RunOutcome::Check { passed: true, .. }));
    let evidence: Value =
        serde_json::from_slice(&fs::read(path.join("compatibility.json")).unwrap()).unwrap();
    assert_eq!(evidence.get("completed").unwrap(), true);
    assert_eq!(evidence.get("findings").unwrap(), false);
    assert!(
        evidence
            .get("packages")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(path.join("report.json").is_file());
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Compares real Cargo dependency graphs across captured report modes"
)]
fn compatibility_reports_preserve_workspace_dependency_graphs() {
    let fixture = Fixture::new("");
    write_package(
        &fixture,
        "dependency",
        "1.0.0",
        "[package.metadata.release-plan]\nprivate-api = true\n",
    );
    write_package(
        &fixture,
        "consumer",
        "1.0.0",
        "[dependencies]\ndep_alias = { package = 'dependency', path = '../dependency', version = '=1.0.0' }\n\
         [package.metadata.release-plan]\nprivate-api = true\n\
         [package.metadata.cargo_check_external_types]\nallowed_external_types = ['dependency::*']\n",
    );
    fixture.commit("workspace dependency graph");
    let output = TempDir::new().unwrap();
    let prepared = output.path().join("prepared");
    run(&RunInput::Prepare {
        output: prepared.clone(),
        release_history: Some(fixture.sha("HEAD")),
        merge_target: None,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    let expected: Value =
        serde_json::from_slice(&fs::read(prepared.join("report.json")).unwrap()).unwrap();
    let packages = expected.get("packages").unwrap().as_array().unwrap();
    let consumer = packages
        .iter()
        .find(|package| package.get("name").unwrap() == "consumer")
        .unwrap();
    assert_eq!(
        consumer.pointer("/dependencies/0/name").unwrap(),
        "dependency"
    );
    assert_eq!(consumer.pointer("/dependencies/0/public").unwrap(), true);
    let dependency = packages
        .iter()
        .find(|package| package.get("name").unwrap() == "dependency")
        .unwrap();
    assert_eq!(dependency.get("dependents").unwrap(), &json!(["consumer"]));
    assert_eq!(consumer.get("group").unwrap(), "consumer");
    assert_eq!(
        expected.pointer("/groups/consumer/members").unwrap(),
        &json!(["consumer", "dependency"])
    );

    for (label, prepared_path, release_history) in [
        ("fresh", None, Some(fixture.sha("HEAD"))),
        ("prepared", Some(prepared.join("prepared.json")), None),
    ] {
        let evidence = output.path().join(label).join("compatibility");
        assert!(matches!(
            run(&RunInput::CheckCompatibility {
                manifest_path: fixture.manifest(),
                prepared: prepared_path,
                plan: None,
                release_history,
                merge_target: None,
                output: evidence.clone(),
                deny_findings: true,
                verbose: false,
            })
            .unwrap(),
            RunOutcome::Check { passed: true, .. }
        ));
        let report: Value =
            serde_json::from_slice(&fs::read(evidence.join("report.json")).unwrap()).unwrap();
        assert_eq!(report, expected);
        let outcome = read_outcome(&evidence);
        assert_eq!(outcome.get("packages").unwrap(), &json!([]));
    }
}

#[test]
#[cfg_attr(miri, ignore = "Prepares and validates a real Cargo/Git workspace")]
fn prepared_compatibility_rejects_same_head_source_drift_before_comparison() {
    let fixture = Fixture::new("");
    // The version is representative; captured-source rejection precedes any baseline comparison.
    write_package(&fixture, "library", "1.0.0", "");
    fixture.commit("prepared source");
    let output = TempDir::new().unwrap();
    let prepared = output.path().join("prepared");
    run(&RunInput::Prepare {
        output: prepared.clone(),
        release_history: Some(fixture.sha("HEAD")),
        merge_target: None,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    fixture.write(
        "packages/library/src/lib.rs",
        "pub fn changed_without_commit() {}\n",
    );
    let evidence = output.path().join("compatibility");
    run(&RunInput::CheckCompatibility {
        manifest_path: fixture.manifest(),
        prepared: Some(prepared.join("prepared.json")),
        plan: None,
        release_history: None,
        merge_target: None,
        output: evidence.clone(),
        deny_findings: false,
        verbose: false,
    })
    .unwrap_err();
    assert!(!evidence.join("compatibility.json").exists());
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Prepares and reclassifies real captured Cargo/Git source"
)]
fn prepared_check_reclassifies_bound_source_instead_of_trusting_adjacent_report() {
    let fixture = private_library();
    fixture.write(
        "packages/library/src/lib.rs",
        "pub fn implementation_change() {}\n",
    );
    let output = TempDir::new().unwrap();
    let prepared = output.path().join("prepared");
    run(&RunInput::Prepare {
        output: prepared.clone(),
        release_history: Some(fixture.sha("HEAD")),
        merge_target: None,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    let expected = fs::read(prepared.join("report.json")).unwrap();
    // The adjacent report is not the source identity carried by prepared.json.
    fs::write(prepared.join("report.json"), b"not a report").unwrap();
    let evidence = output.path().join("evidence");
    let input = RunInput::CheckCompatibility {
        manifest_path: fixture.manifest(),
        prepared: Some(prepared.join("prepared.json")),
        plan: None,
        release_history: None,
        merge_target: None,
        output: evidence.clone(),
        deny_findings: true,
        verbose: true,
    };
    assert!(matches!(
        run(&input).unwrap(),
        RunOutcome::Check { passed: true, .. }
    ));
    assert_eq!(fs::read(evidence.join("report.json")).unwrap(), expected);
    let outcome = read_outcome(&evidence);
    assert_eq!(outcome.get("completed").unwrap(), true);
    assert_eq!(outcome.get("findings").unwrap(), false);
    assert_eq!(outcome.get("packages").unwrap(), &json!([]));
    assert_eq!(
        outcome.get("report").unwrap(),
        &json!(evidence.join("report.json"))
    );
    let receipt = fs::read(evidence.join("compatibility.json")).unwrap();
    run(&input).unwrap_err();
    assert_eq!(
        fs::read(evidence.join("compatibility.json")).unwrap(),
        receipt
    );
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Validates preparation schema and real source identities"
)]
fn prepared_check_rejects_unknown_schema_and_another_workspace() {
    let fixture = private_library();
    let other = Fixture::from_template(&fixture);
    let output = TempDir::new().unwrap();
    let prepared = output.path().join("prepared");
    run(&RunInput::Prepare {
        output: prepared.clone(),
        release_history: Some(fixture.sha("HEAD")),
        merge_target: None,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    let path = prepared.join("prepared.json");
    let mut artifact: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let current_schema = artifact.get("schema_version").unwrap().clone();
    *artifact.get_mut("schema_version").unwrap() = json!(u32::MAX);
    fs::write(&path, serde_json::to_vec(&artifact).unwrap()).unwrap();
    let invalid = output.path().join("invalid-schema");
    run(&RunInput::CheckCompatibility {
        manifest_path: fixture.manifest(),
        prepared: Some(path.clone()),
        plan: None,
        release_history: None,
        merge_target: None,
        output: invalid.clone(),
        deny_findings: false,
        verbose: false,
    })
    .unwrap_err();
    assert!(!invalid.join("compatibility.json").exists());
    assert!(!invalid.join("report.json").exists());
    *artifact.get_mut("schema_version").unwrap() = current_schema;
    fs::write(&path, serde_json::to_vec(&artifact).unwrap()).unwrap();
    let wrong_source = output.path().join("wrong-source");
    run(&RunInput::CheckCompatibility {
        manifest_path: other.manifest(),
        prepared: Some(path),
        plan: None,
        release_history: None,
        merge_target: None,
        output: wrong_source.clone(),
        deny_findings: false,
        verbose: false,
    })
    .unwrap_err();
    assert!(!wrong_source.join("compatibility.json").exists());
    assert!(!wrong_source.join("report.json").exists());
}

#[test]
#[cfg_attr(miri, ignore = "Previews and verifies retained Cargo/Git source")]
fn preview_check_uses_final_source_and_rejects_drift_in_either_workspace() {
    let fixture = private_library();
    fixture.write(
        "proposal.json",
        r#"{"schema_version":6,"increments":[{"name":"library","bump":"patch"}]}"#,
    );
    let plan = resolved_plan(&fixture, &fixture.path().join("proposal.json"));
    let output = TempDir::new().unwrap();
    let evidence = output.path().join("evidence");
    let input = |output: PathBuf| RunInput::CheckCompatibility {
        manifest_path: fixture.manifest(),
        prepared: None,
        plan: Some(plan.clone()),
        release_history: None,
        merge_target: None,
        output,
        deny_findings: true,
        verbose: true,
    };
    let original_manifest = fixture.read("packages/library/Cargo.toml");
    assert!(matches!(
        run(&input(evidence.clone())).unwrap(),
        RunOutcome::Check { passed: true, .. }
    ));
    assert_eq!(read_outcome(&evidence).get("completed").unwrap(), true);
    let report: Value =
        serde_json::from_slice(&fs::read(evidence.join("report.json")).unwrap()).unwrap();
    assert_eq!(
        report.pointer("/packages/0/declared_version").unwrap(),
        "1.0.1"
    );
    assert_eq!(
        fixture.read("packages/library/Cargo.toml"),
        original_manifest
    );

    for source in [
        fixture
            .path()
            .join("preview/workspace/packages/library/src/lib.rs"),
        fixture.path().join("packages/library/src/lib.rs"),
    ] {
        let original = fs::read(&source).unwrap();
        fs::write(&source, "pub fn drift_after_preview() {}\n").unwrap();
        let rejected = output
            .path()
            .join(if source.starts_with(fixture.path().join("preview")) {
                "candidate-drift"
            } else {
                "source-drift"
            });
        run(&input(rejected.clone())).unwrap_err();
        assert!(!rejected.join("report.json").exists());
        assert!(!rejected.join("compatibility.json").exists());
        fs::write(source, original).unwrap();
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Validates an unresolved plan against a real Cargo/Git workspace"
)]
fn compatibility_requires_resolved_plan_evidence() {
    let fixture = private_library();
    fixture.write(
        "proposal.json",
        r#"{"schema_version":6,"increments":[{"name":"library","bump":"patch"}]}"#,
    );
    let output = TempDir::new().unwrap();
    let evidence = output.path().join("evidence");
    run(&RunInput::CheckCompatibility {
        manifest_path: fixture.manifest(),
        prepared: None,
        plan: Some(fixture.path().join("proposal.json")),
        release_history: None,
        merge_target: None,
        output: evidence.clone(),
        deny_findings: false,
        verbose: false,
    })
    .unwrap_err();
    assert!(!evidence.join("report.json").exists());
    assert!(!evidence.join("compatibility.json").exists());
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Starts Cargo with a controlled external checker executable"
)]
fn checker_failures_leave_incomplete_evidence_and_preserve_diagnostics() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let fixture = Fixture::new("");
        write_package(&fixture, "library", "1.0.0", "");
        // A candidate's Cargo alias must not substitute the installed checker at any phase.
        fixture.write(
            ".cargo/config.toml",
            "[alias]\nsemver-checks = 'not-the-installed-checker'\n",
        );
        fixture.commit("released library");
        fixture.write(
            "packages/library/src/lib.rs",
            "pub fn pending_change() {}\n",
        );
        let output = TempDir::new().unwrap();
        for scenario in ["identity-failure", "canary-failure", "source-drift"] {
            let evidence = output.path().join(scenario);
            let invocations = output.path().join(format!("{scenario}.calls"));
            let original = fixture.read("packages/library/src/lib.rs");
            let mut command = checker_command();
            command
                .args(["check-compatibility", "--manifest-path"])
                .arg(fixture.manifest())
                .args(["--release-history", &fixture.sha("HEAD"), "--output"])
                .arg(&evidence)
                .arg("--deny-findings")
                .env("CRP_FIXTURE_SCENARIO", scenario)
                .env("CRP_FIXTURE_CALLS", &invocations)
                .env(
                    "CRP_FIXTURE_SOURCE",
                    fixture.path().join("packages/library/src/lib.rs"),
                );
            for credential in [
                "GH_TOKEN",
                "GITHUB_TOKEN",
                "GIT_TOKEN",
                "INPUT_TOKEN",
                "DEFAULT_GITHUB_TOKEN",
                "CARGO_REGISTRY_TOKEN",
                "CARGO_REGISTRIES_CRATES_IO_TOKEN",
                "CARGO_REGISTRIES_PRIVATE_TOKEN",
                "ACTIONS_ID_TOKEN_REQUEST_URL",
                "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
            ] {
                command.env(credential, "must-not-reach-checker");
            }
            let result = command.output().unwrap();
            assert!(!result.status.success());
            assert!(result.stdout.is_empty());
            let outcome = read_outcome(&evidence);
            assert_eq!(outcome.get("completed").unwrap(), false);
            assert_eq!(outcome.get("findings").unwrap(), false);
            assert_eq!(outcome.get("packages").unwrap(), &json!([]));
            assert!(evidence.join("report.json").is_file());
            if scenario == "identity-failure" {
                assert_eq!(
                    outcome.get("checker").unwrap(),
                    "selected: checker identity unavailable"
                );
                assert_eq!(fs::read_to_string(invocations).unwrap(), "version\n");
                assert!(
                    fs::read(evidence.join("semver-checks.log"))
                        .unwrap()
                        .is_empty()
                );
            } else {
                assert_eq!(
                    fs::read_to_string(invocations).unwrap(),
                    "version\ncanary\n"
                );
                assert_eq!(
                    outcome.get("checker").unwrap(),
                    "cargo-semver-checks 0.50.0 (fixture)"
                );
                assert_eq!(
                    fs::read_to_string(evidence.join("semver-checks.log")).unwrap(),
                    "canary stdout\ncanary stderr\n"
                );
            }
            if scenario == "source-drift" {
                let diagnostic = String::from_utf8_lossy(&result.stderr);
                assert!(diagnostic.contains("identical-source canary"));
                assert!(diagnostic.contains("source verification also failed"));
                assert_ne!(fixture.read("packages/library/src/lib.rs"), original);
                fixture.write("packages/library/src/lib.rs", &original);
            } else {
                assert_eq!(fixture.read("packages/library/src/lib.rs"), original);
            }
        }
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Runs the application with a non-executable checker fixture"
)]
fn checker_start_failure_retains_selected_but_unidentified_state() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let fixture = Fixture::new("");
        write_package(&fixture, "library", "1.0.0", "");
        fixture.commit("released library");
        fixture.write(
            "packages/library/src/lib.rs",
            "pub fn pending_change() {}\n",
        );
        let output = TempDir::new().unwrap();
        let tools = output.path().join("tools");
        fs::create_dir_all(&tools).unwrap();
        let checker = tools.join(format!("cargo-semver-checks{EXE_SUFFIX}"));
        fs::write(&checker, b"not an executable image").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(&checker, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = env::join_paths(
            iter::once(tools).chain(env::split_paths(&env::var_os("PATH").unwrap())),
        )
        .unwrap();
        let evidence = output.path().join("evidence");
        let result = Command::new(env!("CARGO_BIN_EXE_cargo-release-plan"))
            .args(["check-compatibility", "--manifest-path"])
            .arg(fixture.manifest())
            .args(["--release-history", &fixture.sha("HEAD"), "--output"])
            .arg(&evidence)
            .env("PATH", path)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("failed to start cargo-semver-checks")
        );
        let outcome = read_outcome(&evidence);
        assert_eq!(outcome.get("completed").unwrap(), false);
        assert_eq!(
            outcome.get("checker").unwrap(),
            "selected: checker identity unavailable"
        );
        assert_eq!(outcome.get("packages").unwrap(), &json!([]));
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Mutates tracked source during real report history acquisition"
)]
fn fresh_source_drift_during_report_prevents_checker_invocation_and_evidence_acceptance() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let fixture = Fixture::new("");
        write_package(&fixture, "library", "1.0.0", "");
        fixture.commit("captured source");
        let output = TempDir::new().unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-release-plan"));
        configure_git_shim(&mut command, output.path());
        let calls = output.path().join("checker-calls");
        let marker = output.path().join("report-drift");
        let evidence = output.path().join("evidence");
        let result = command
            .args(["check-compatibility", "--manifest-path"])
            .arg(fixture.manifest())
            .args(["--release-history", &fixture.sha("HEAD"), "--output"])
            .arg(&evidence)
            .env("CRP_REPORT_DRIFT_MARKER", &marker)
            .env(
                "CRP_FIXTURE_SOURCE",
                fixture.path().join("packages/library/src/lib.rs"),
            )
            .env("CRP_FIXTURE_SCENARIO", "canary-failure")
            .env("CRP_FIXTURE_CALLS", &calls)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(marker.is_file());
        assert!(evidence.join("report.json").is_file());
        assert!(String::from_utf8_lossy(&result.stderr).contains("inputs are stale"));
        assert!(!calls.exists());
        assert!(!evidence.join("compatibility.json").exists());
        assert!(
            fixture
                .read("packages/library/src/lib.rs")
                .contains("report_drift")
        );
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Uses real Git snapshots and a local checker protocol executable"
)]
fn anticipated_parent_final_api_is_the_child_baseline_without_registry_access() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let (fixture, history, parent) = anticipated_parent();
        let output = TempDir::new().unwrap();
        let unchanged = output.path().join("unchanged");
        let unchanged_calls = output.path().join("unchanged.calls");
        let result = parent_check(&fixture, &history, &unchanged, &unchanged_calls)
            .env("CRP_FIXTURE_SCENARIO", "anticipated-parent")
            .env("CRP_EXPECTED_PARENT", &parent)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(
            read_outcome(&unchanged).get("packages").unwrap(),
            &json!([])
        );
        assert!(!unchanged_calls.exists());

        // The parent added this API only after its version-edit commit. The child removes it;
        // comparing against the published history alone would miss the parent's new contract.
        write_package(&fixture, "library", "1.1.1", "");
        fixture.write("packages/library/src/lib.rs", "pub fn existing() {}\n");
        fixture.commit("child removes anticipated parent API");
        let evidence = output.path().join("removed");
        let calls = output.path().join("removed.calls");
        let result = parent_check(&fixture, &history, &evidence, &calls)
            .env("CRP_FIXTURE_SCENARIO", "anticipated-parent")
            .env("CRP_EXPECTED_PARENT", &parent)
            .output()
            .unwrap();
        assert!(!result.status.success());
        let outcome = read_outcome(&evidence);
        assert_eq!(outcome.get("completed").unwrap(), true);
        assert_eq!(outcome.get("findings").unwrap(), true);
        let comparison = outcome
            .get("packages")
            .unwrap()
            .as_array()
            .unwrap()
            .first()
            .unwrap();
        assert_eq!(comparison.get("baseline_version").unwrap(), "1.1.0");
        assert_eq!(comparison.get("required_impact").unwrap(), "breaking");
        assert_eq!(comparison.get("compared").unwrap(), true);
        assert_eq!(
            fs::read_to_string(&calls).unwrap(),
            "version\ncanary\ncomparison\n"
        );
        assert_eq!(
            fixture
                .git(&["worktree", "list", "--porcelain"])
                .matches("worktree ")
                .count(),
            1
        );
        assert_eq!(fixture.sha("anticipated-parent"), parent);
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Compares a relocated member against its original parent workspace"
)]
fn anticipated_parent_baseline_uses_workspace_root_for_a_moved_member_manifest() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let (fixture, history, parent) = anticipated_parent();
        fixture.write("packages/library/src/lib.rs", "pub fn existing() {}\n");
        let moved = fixture.path().join("packages").join("relocated");
        fs::rename(fixture.path().join("packages").join("library"), &moved).unwrap();
        fixture.commit("child moves member and removes parent API");
        let output = TempDir::new().unwrap();
        let evidence = output.path().join("evidence");
        let calls = output.path().join("calls");
        let result = checker_command()
            .args(["check-compatibility", "--manifest-path"])
            .arg(moved.join("Cargo.toml"))
            .args([
                "--release-history",
                &history,
                "--merge-target",
                "anticipated-parent",
                "--output",
            ])
            .arg(&evidence)
            .arg("--deny-findings")
            .env("CRP_FIXTURE_SCENARIO", "anticipated-parent")
            .env("CRP_EXPECTED_PARENT", &parent)
            .env("CRP_FIXTURE_CALLS", &calls)
            .env("CRP_FIXTURE_SOURCE", moved.join("src").join("lib.rs"))
            .output()
            .unwrap();
        assert!(!result.status.success());
        let outcome = read_outcome(&evidence);
        assert_eq!(outcome.get("completed").unwrap(), true);
        assert_eq!(
            outcome.pointer("/packages/0/baseline_version").unwrap(),
            "1.1.0"
        );
        assert_eq!(
            outcome.pointer("/packages/0/required_impact").unwrap(),
            "breaking"
        );
        assert_eq!(
            fs::read_to_string(calls).unwrap(),
            "version\ncanary\ncomparison\n"
        );
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Moves an anticipated ref after captured preparation and preview"
)]
fn moved_anticipated_parent_invalidates_captured_compatibility_before_checker() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let (fixture, history, parent) = anticipated_parent();
        fixture.write("packages/library/src/lib.rs", "pub fn existing() {}\n");
        let output = TempDir::new().unwrap();
        let prepared = output.path().join("prepared");
        run(&RunInput::Prepare {
            output: prepared.clone(),
            release_history: Some(history.clone()),
            merge_target: Some("anticipated-parent".to_owned()),
            manifest_path: fixture.manifest(),
            verbose: false,
        })
        .unwrap();
        let proposal = output.path().join("proposal.json");
        fs::write(
            &proposal,
            r#"{"schema_version":6,"increments":[{"name":"library","bump":"major"}]}"#,
        )
        .unwrap();
        let preview = output.path().join("preview");
        run(&RunInput::Preview {
            plan: proposal,
            prepared: prepared.join("prepared.json"),
            output: preview.clone(),
            manifest_path: fixture.manifest(),
            verbose: false,
        })
        .unwrap();
        fixture.git(&[
            "update-ref",
            "refs/heads/anticipated-parent",
            &history,
            &parent,
        ]);
        for (option, artifact) in [
            ("--prepared", prepared.join("prepared.json")),
            ("--plan", preview.join("plan.json")),
        ] {
            let evidence = output.path().join(option);
            let calls = output.path().join(format!("{option}.calls"));
            let result = checker_command()
                .args(["check-compatibility", "--manifest-path"])
                .arg(fixture.manifest())
                .arg(option)
                .arg(artifact)
                .arg("--output")
                .arg(&evidence)
                .env("CRP_FIXTURE_SCENARIO", "anticipated-parent")
                .env("CRP_EXPECTED_PARENT", &parent)
                .env("CRP_FIXTURE_CALLS", &calls)
                .output()
                .unwrap();
            assert!(!result.status.success());
            assert!(!calls.exists());
            assert!(!evidence.join("compatibility.json").exists());
        }
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Mutates parent comparison inputs through a real checker fixture"
)]
fn anticipated_parent_source_and_target_drift_invalidate_comparison_evidence() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let (fixture, history, parent) = anticipated_parent();
        fixture.write("packages/library/src/lib.rs", "pub fn existing() {}\n");
        fixture.commit("child removes anticipated API");
        let output = TempDir::new().unwrap();
        for scenario in [
            "parent-source-drift",
            "parent-head-drift",
            "parent-target-drift",
        ] {
            let evidence = output.path().join(scenario);
            let calls = output.path().join(format!("{scenario}.calls"));
            let result = parent_check(&fixture, &history, &evidence, &calls)
                .env("CRP_FIXTURE_SCENARIO", scenario)
                .env("CRP_EXPECTED_PARENT", &parent)
                .env("CRP_FIXTURE_HISTORY", &history)
                .env("CRP_FIXTURE_ROOT", fixture.path())
                .output()
                .unwrap();
            assert!(!result.status.success());
            let outcome = read_outcome(&evidence);
            assert_eq!(outcome.get("completed").unwrap(), false);
            let invocations = fs::read_to_string(&calls).unwrap();
            if scenario != "parent-target-drift" {
                assert_eq!(invocations, "version\ncanary\ncomparison\n");
                assert_eq!(outcome.get("findings").unwrap(), true);
                let diagnostic = if scenario == "parent-head-drift" {
                    "immutable source HEAD"
                } else {
                    "unchanged source"
                };
                assert!(String::from_utf8_lossy(&result.stderr).contains(diagnostic));
                assert_eq!(fixture.sha("anticipated-parent"), parent);
            } else {
                assert_eq!(invocations, "version\ncanary\n");
                assert_eq!(outcome.get("packages").unwrap(), &json!([]));
                assert_eq!(fixture.sha("anticipated-parent"), history);
                fixture.git(&[
                    "update-ref",
                    "refs/heads/anticipated-parent",
                    &parent,
                    &history,
                ]);
            }
            assert_eq!(
                fixture
                    .git(&["worktree", "list", "--porcelain"])
                    .matches("worktree ")
                    .count(),
                1
            );
        }
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Runs parent comparison failures and owned Git worktree cleanup"
)]
fn anticipated_parent_comparison_and_cleanup_failures_remain_explicit() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        // The checker consumes the nested Cargo workspace, but cleanup owns its enclosing
        // registered Git worktree. Those paths must not be conflated.
        let (fixture, history, parent) = anticipated_parent_in("nested-workspace/");
        fixture.write(
            "nested-workspace/packages/library/src/lib.rs",
            "pub fn existing() {}\n",
        );
        fixture.commit("child removes anticipated API");
        let output = TempDir::new().unwrap();
        #[cfg(windows)]
        let temporary = tempfile::Builder::new()
            .prefix("compatibility temporary alias ")
            .tempdir()
            .unwrap();
        #[cfg(windows)]
        let short_temporary = windows_short_directory(temporary.path());
        for scenario in [
            "parent-comparison-failure",
            "parent-comparison-and-cleanup-failure",
        ] {
            let evidence = output.path().join(scenario);
            let calls = output.path().join(format!("{scenario}.calls"));
            let baseline = output.path().join(format!("{scenario}.baseline"));
            let mut command = parent_check(&fixture, &history, &evidence, &calls);
            #[cfg(windows)]
            if let Some(path) = &short_temporary {
                // Only this child uses the alias; parallel tests retain their own environment.
                command.env("TEMP", path).env("TMP", path);
            }
            let result = command
                .env("CRP_FIXTURE_SCENARIO", scenario)
                .env("CRP_EXPECTED_PARENT", &parent)
                .env("CRP_FIXTURE_BASELINE_PATH", &baseline)
                .output()
                .unwrap();
            assert!(!result.status.success());
            assert_eq!(read_outcome(&evidence).get("completed").unwrap(), false);
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert!(stderr.contains("cargo-semver-checks failed"));
            if scenario == "parent-comparison-and-cleanup-failure" {
                assert!(stderr.contains("source cleanup also failed"));
                assert!(stderr.contains("comparison failure canary"));
                // The checker captured Git's registered root while it existed. TempDir cleanup
                // removes the source even when Git refuses removal of a locked registration.
                let registered = PathBuf::from(fs::read_to_string(&baseline).unwrap());
                let supplied =
                    PathBuf::from(fs::read_to_string(baseline.with_extension("supplied")).unwrap());
                assert!(!registered.exists());
                assert!(!registered.parent().unwrap().exists());
                fixture.git(&["worktree", "unlock", registered.to_str().unwrap()]);
                fixture.git(&["worktree", "prune"]);
                let supplied: PathBuf = supplied.components().collect();
                assert_eq!(supplied, registered.join("nested-workspace"));
            }
            assert_eq!(
                fixture
                    .git(&["worktree", "list", "--porcelain"])
                    .matches("worktree ")
                    .count(),
                1
            );
        }
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Injects a failed native Git result after worktree registration"
)]
fn failed_parent_worktree_add_still_cleans_its_registered_source() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let (fixture, release_history, parent) = anticipated_parent();
        fixture.write("packages/library/src/lib.rs", "pub fn existing() {}\n");
        fixture.commit("child removes anticipated API");
        let output = TempDir::new().unwrap();
        let evidence = output.path().join("evidence");
        let calls = output.path().join("calls");
        let marker = output.path().join("partial-add");
        let mut command = parent_check(&fixture, &release_history, &evidence, &calls);
        configure_git_shim(&mut command, output.path());
        let result = command
            .env("CRP_FIXTURE_SCENARIO", "anticipated-parent")
            .env("CRP_EXPECTED_PARENT", &parent)
            .env("CRP_PARENT_ADD_FAILURE", &marker)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(marker.is_file());
        assert!(marker.with_extension("removed").is_file());
        assert!(!calls.exists());
        assert_eq!(read_outcome(&evidence).get("completed").unwrap(), false);
        assert!(String::from_utf8_lossy(&result.stderr).contains("partial-add failure canary"));
        assert_eq!(
            fixture
                .git(&["worktree", "list", "--porcelain"])
                .matches("worktree ")
                .count(),
            1,
        );
    });
}

fn anticipated_parent() -> (Fixture, String, String) {
    anticipated_parent_in("")
}

fn anticipated_parent_in(prefix: &str) -> (Fixture, String, String) {
    let fixture = Fixture::with_workspace_manifest(
        &format!("{prefix}Cargo.toml"),
        "[workspace]\nmembers = ['packages/*']\nresolver = '2'\n",
    );
    let manifest = format!("{prefix}packages/library/Cargo.toml");
    let source = format!("{prefix}packages/library/src/lib.rs");
    fixture.write(
        &manifest,
        "[package]\nname = 'library'\nversion = '1.0.0'\nedition = '2021'\n",
    );
    fixture.write(&source, "pub fn existing() {}\n");
    fixture.commit("published history without parent API");
    let history = fixture.sha("HEAD");
    fixture.git(&["checkout", "-b", "anticipated-parent"]);
    fixture.write(
        &manifest,
        "[package]\nname = 'library'\nversion = '1.1.0'\nedition = '2021'\n",
    );
    fixture.commit("parent version edit");
    fixture.write(&source, "pub fn existing() {}\npub fn parent_added() {}\n");
    fixture.commit("parent final API");
    let parent = fixture.sha("HEAD");
    fixture.git(&["checkout", "-b", "child"]);
    (fixture, history, parent)
}

fn parent_check(fixture: &Fixture, history: &str, evidence: &Path, calls: &Path) -> Command {
    let mut command = checker_command();
    command
        .args(["check-compatibility", "--manifest-path"])
        .arg(fixture.manifest())
        .args([
            "--release-history",
            history,
            "--merge-target",
            "anticipated-parent",
            "--output",
        ])
        .arg(evidence)
        .arg("--deny-findings")
        .env(
            "CRP_FIXTURE_SOURCE",
            fixture
                .manifest()
                .parent()
                .unwrap()
                .join("packages/library/src/lib.rs"),
        )
        .env("CRP_FIXTURE_CALLS", calls);
    command
}

#[cfg(windows)]
fn windows_short_directory(path: &Path) -> Option<PathBuf> {
    // Probe the actual filesystem's short-name support using the existing fixture approach.
    // No volume setting, global environment or case-sensitivity assumption is changed.
    let script = path.join("short-path.ps1");
    fs::write(
        &script,
        "# Returns the actual short-name spelling for this fixture's owned temporary directory.\n\
         param([string] $Path)\n\
         Set-StrictMode -Version Latest\n\
         $ErrorActionPreference = 'Stop'\n\
         $PSNativeCommandUseErrorActionPreference = $true\n\
         $filesystem = New-Object -ComObject Scripting.FileSystemObject\n\
         $filesystem.GetFolder($Path).ShortPath\n",
    )
    .unwrap();
    let output = Command::new("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(&script)
        .arg(path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let short = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
    assert_eq!(
        fs::canonicalize(&short).unwrap(),
        fs::canonicalize(path).unwrap()
    );
    if short == path {
        eprintln!("The fixture filesystem exposes no distinct short directory name.");
        None
    } else {
        Some(short)
    }
}

fn private_library() -> Fixture {
    let fixture = Fixture::new("");
    write_package(
        &fixture,
        "library",
        "1.0.0",
        "[package.metadata.release-plan]\nprivate-api = true\n",
    );
    fixture.commit("released implementation");
    fixture
}

fn read_outcome(output: &Path) -> Value {
    let outcome: Value =
        serde_json::from_slice(&fs::read(output.join("compatibility.json")).unwrap()).unwrap();
    assert_eq!(outcome.get("schema_version").unwrap(), 2);
    outcome
}

fn configure_git_shim(command: &mut Command, output: &Path) {
    let tools = output.join("git-shim");
    fs::create_dir_all(&tools).unwrap();
    let real_git = env::split_paths(&env::var_os("PATH").unwrap())
        .map(|directory| directory.join(format!("git{EXE_SUFFIX}")))
        .find(|candidate| candidate.is_file())
        .unwrap()
        .canonicalize()
        .unwrap();
    fs::copy(
        CHECKER
            .path()
            .join(format!("cargo-semver-checks{EXE_SUFFIX}")),
        tools.join(format!("git{EXE_SUFFIX}")),
    )
    .unwrap();
    let path = env::join_paths(
        [tools, CHECKER.path().to_path_buf()]
            .into_iter()
            .chain(env::split_paths(&env::var_os("PATH").unwrap())),
    )
    .unwrap();
    command.env("PATH", path).env("CRP_REAL_GIT", real_git);
}

fn checker_command() -> Command {
    let path = env::join_paths(
        iter::once(CHECKER.path().to_path_buf())
            .chain(env::split_paths(&env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-release-plan"));
    command.env("PATH", path);
    command
}

/// A Cargo subcommand fixture supplies deterministic failures or parent-source API evidence.
///
/// It exercises the actual process arguments, environment and canary files. Comparison
/// decisions over acquired checker output remain in the implementation's unit tests.
static CHECKER: LazyLock<TempDir> = LazyLock::new(|| {
    let directory = TempDir::new().unwrap();
    let source = directory.path().join("checker.rs");
    fs::write(&source, r#"
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::process;

fn git() -> process::Command {
    let mut command = process::Command::new("git");
    command.args(["-c", "user.name=Compatibility Fixture", "-c", "user.email=compatibility@example.invalid",
        "-c", "commit.gpgsign=false", "-c", "gc.auto=0"]);
    command
}

fn main() {
    let os_args: Vec<_> = env::args_os().collect();
    if Path::new(&os_args[0]).file_stem().is_some_and(|name| name == "git") {
        // Forward actual Git output unchanged. Historical tree acquisition belongs to report,
        // not Inputs::capture; this event injects drift without invocation counts or clocks.
        let status = process::Command::new(env::var_os("CRP_REAL_GIT").unwrap())
            .args(&os_args[1..]).status().unwrap();
        if status.success() {
            if let Some(marker) = env::var_os("CRP_PARENT_ADD_FAILURE") {
                if os_args.windows(2).any(|pair| pair[0] == "worktree" && pair[1] == "add") {
                    fs::write(&marker, "registered").unwrap();
                    eprintln!("partial-add failure canary");
                    process::exit(1);
                }
                if os_args.windows(2).any(|pair| pair[0] == "worktree" && pair[1] == "remove") {
                    fs::write(Path::new(&marker).with_extension("removed"), "removed").unwrap();
                }
            }
        }
        if status.success() && env::var_os("CRP_REPORT_DRIFT_MARKER").is_some()
            && os_args.iter().any(|arg| arg == "ls-tree") {
            match OpenOptions::new().write(true).create_new(true)
                .open(env::var_os("CRP_REPORT_DRIFT_MARKER").unwrap()) {
                Ok(_) => fs::write(env::var_os("CRP_FIXTURE_SOURCE").unwrap(),
                    "pub fn report_drift() {}\n").unwrap(),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => panic!("{error}"),
            }
        }
        process::exit(status.code().unwrap_or(1));
    }
    let args: Vec<_> = env::args().collect();
    if env::var_os("CRP_FIXTURE_PROBE").is_some() {
        println!("cargo-semver-checks 0.50.0 (fixture)");
        return;
    }
    for name in ["GH_TOKEN", "GITHUB_TOKEN", "GIT_TOKEN", "INPUT_TOKEN", "DEFAULT_GITHUB_TOKEN",
        "CARGO_REGISTRY_TOKEN", "CARGO_REGISTRIES_CRATES_IO_TOKEN", "CARGO_REGISTRIES_PRIVATE_TOKEN",
        "ACTIONS_ID_TOKEN_REQUEST_URL", "ACTIONS_ID_TOKEN_REQUEST_TOKEN"] {
        assert!(env::var_os(name).is_none());
    }
    assert_eq!(env::var("CARGO_TERM_COLOR").unwrap(), "never");
    assert!(Path::new(&env::var_os("CARGO_TARGET_DIR").unwrap()).is_dir());
    let scenario = env::var("CRP_FIXTURE_SCENARIO").unwrap();
    let mut calls = OpenOptions::new().create(true).append(true)
        .open(env::var_os("CRP_FIXTURE_CALLS").unwrap()).unwrap();
    if args.iter().any(|arg| arg == "--version") {
        writeln!(calls, "version").unwrap();
        if scenario == "identity-failure" {
            process::exit(1);
        }
        println!("cargo-semver-checks 0.50.0 (fixture)");
        return;
    }
    let value = |option| args.windows(2).find(|pair| pair[0] == option).unwrap()[1].clone();
    let manifest = value("--manifest-path");
    let baseline = value("--baseline-root");
    if args.iter().any(|arg| arg == "-p") {
        writeln!(calls, "comparison").unwrap();
        assert_eq!(value("-p"), "library");
        assert!(!args.iter().any(|arg| arg == "--baseline-version"));
        assert!(args.iter().any(|arg| arg == "--all-features"));
        let parent = git().args(["-C", &baseline, "rev-parse", "HEAD"]).output().unwrap();
        assert!(parent.status.success());
        assert_eq!(String::from_utf8(parent.stdout).unwrap().trim(), env::var("CRP_EXPECTED_PARENT").unwrap());
        let parent_source = fs::read_to_string(Path::new(&baseline).join("packages/library/src/lib.rs")).unwrap();
        assert!(parent_source.contains("pub fn parent_added()"));
        let parent_manifest = fs::read_to_string(Path::new(&baseline).join("packages/library/Cargo.toml")).unwrap();
        assert!(parent_manifest.contains("1.1.0"));
        let current = fs::read_to_string(env::var_os("CRP_FIXTURE_SOURCE").unwrap()).unwrap();
        assert!(!current.contains("pub fn parent_added()"));
        if scenario == "parent-comparison-and-cleanup-failure" {
            let root = git().args(["-C", &baseline, "rev-parse", "--show-toplevel"]).output().unwrap();
            assert!(root.status.success());
            let root = String::from_utf8(root.stdout).unwrap();
            let root = root.trim();
            let record = env::var_os("CRP_FIXTURE_BASELINE_PATH").unwrap();
            fs::write(&record, root).unwrap();
            fs::write(Path::new(&record).with_extension("supplied"), &baseline).unwrap();
            let status = git()
                .args(["-C", root, "worktree", "lock", "--reason", "cleanup fixture", root])
                .status().unwrap();
            assert!(status.success());
        }
        if scenario == "parent-source-drift" {
            fs::write(Path::new(&baseline).join("packages/library/src/lib.rs"),
                "pub fn altered_parent_source() {}\n").unwrap();
        }
        if scenario == "parent-head-drift" {
            // Move only the owned detached worktree's HEAD, leaving the original parent ref
            // and source files unchanged so immutable-HEAD verification is the failing boundary.
            let symbolic = git().args(["-C", &baseline, "symbolic-ref", "--quiet", "HEAD"])
                .output().unwrap();
            assert!(!symbolic.status.success());
            let status = git().args(["-C", &baseline, "update-ref", "--no-deref", "HEAD",
                &env::var("CRP_FIXTURE_HISTORY").unwrap(), &env::var("CRP_EXPECTED_PARENT").unwrap()])
                .status().unwrap();
            assert!(status.success());
        }
        if scenario.starts_with("parent-comparison-") {
            eprintln!("comparison failure canary");
            process::exit(1);
        }
        println!("Summary semver requires new major version");
        process::exit(100);
    }
    assert_eq!(Path::new(&manifest).parent().unwrap(), Path::new(&baseline));
    assert!(Path::new(&manifest).is_file());
    assert!(Path::new(&baseline).join("lib.rs").is_file());
    assert!(args.iter().any(|arg| arg == "--all-features"));
    writeln!(calls, "canary").unwrap();
    if scenario == "parent-target-drift" {
        let status = git().args(["-C", &env::var("CRP_FIXTURE_ROOT").unwrap(),
            "update-ref", "refs/heads/anticipated-parent", &env::var("CRP_FIXTURE_HISTORY").unwrap(),
            &env::var("CRP_EXPECTED_PARENT").unwrap()]).status().unwrap();
        assert!(status.success());
    }
    if scenario == "anticipated-parent" || scenario.starts_with("parent-") {
        println!("Summary no semver update required");
        return;
    }
    if scenario == "source-drift" {
        fs::write(env::var_os("CRP_FIXTURE_SOURCE").unwrap(), "pub fn unexpected_drift() {}\n").unwrap();
    }
    println!("canary stdout");
    eprintln!("canary stderr");
    process::exit(1);
}
"#).unwrap();
    let result = Command::new("rustc")
        .arg("--edition=2024")
        .arg(&source)
        .arg("-o")
        .arg(
            directory
                .path()
                .join(format!("cargo-semver-checks{EXE_SUFFIX}")),
        )
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let path = env::join_paths(
        iter::once(directory.path().to_path_buf())
            .chain(env::split_paths(&env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let result = Command::new(
        directory
            .path()
            .join(format!("cargo-semver-checks{EXE_SUFFIX}")),
    )
    .arg("--version")
    .env("PATH", path)
    .env("CRP_FIXTURE_PROBE", "1")
    .output()
    .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(
        String::from_utf8(result.stdout).unwrap().trim(),
        "cargo-semver-checks 0.50.0 (fixture)"
    );
    directory
});
