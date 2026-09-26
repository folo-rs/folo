//! Captured-source binding and checker failures without production registry access.

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

#[test]
#[cfg_attr(miri, ignore = "Reads real captured source and runs Cargo metadata")]
fn unchanged_workspace_needs_no_checker_or_registry_and_keeps_fresh_report() {
    let fixture = Fixture::new("");
    write_package(&fixture, "library", "1.0.0", "");
    fixture.commit("unchanged source");
    let output = TempDir::new().unwrap();
    let path = output.path().join("evidence");
    let result = run(&RunInput::CheckCompatibility {
        manifest_path: fixture.manifest(),
        prepared: None,
        plan: None,
        base: Some(fixture.sha("HEAD")),
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
        base: Some(fixture.sha("HEAD")),
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

    for (label, prepared_path, base) in [
        ("fresh", None, Some(fixture.sha("HEAD"))),
        ("prepared", Some(prepared.join("prepared.json")), None),
    ] {
        let evidence = output.path().join(label).join("compatibility");
        assert!(matches!(
            run(&RunInput::CheckCompatibility {
                manifest_path: fixture.manifest(),
                prepared: prepared_path,
                plan: None,
                base,
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
    write_package(&fixture, "library", "1.0.0", "");
    fixture.commit("prepared source");
    let output = TempDir::new().unwrap();
    let prepared = output.path().join("prepared");
    run(&RunInput::Prepare {
        output: prepared.clone(),
        base: Some(fixture.sha("HEAD")),
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
        base: None,
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
        base: Some(fixture.sha("HEAD")),
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
        base: None,
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
        base: Some(fixture.sha("HEAD")),
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
        base: None,
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
        base: None,
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
        r#"{"schema_version":4,"increments":[{"name":"library","level":"patch"}]}"#,
    );
    let plan = resolved_plan(&fixture, &fixture.path().join("proposal.json"));
    let output = TempDir::new().unwrap();
    let evidence = output.path().join("evidence");
    let input = |output: PathBuf| RunInput::CheckCompatibility {
        manifest_path: fixture.manifest(),
        prepared: None,
        plan: Some(plan.clone()),
        base: None,
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
        r#"{"schema_version":4,"increments":[{"name":"library","level":"patch"}]}"#,
    );
    let output = TempDir::new().unwrap();
    let evidence = output.path().join("evidence");
    run(&RunInput::CheckCompatibility {
        manifest_path: fixture.manifest(),
        prepared: None,
        plan: Some(fixture.path().join("proposal.json")),
        base: None,
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
    ignore = "Discovers and previews real nonpublishable Cargo/Git members"
)]
fn publication_preflight_does_not_query_or_change_nonpublishable_members() {
    let fixture = Fixture::new("");
    write_package(&fixture, "helper", "1.0.0", "publish = false\n");
    fixture.commit("local-only workspace");
    fixture.write("proposal.json", r#"{"schema_version":4,"increments":[]}"#);
    let plan = resolved_plan(&fixture, &fixture.path().join("proposal.json"));
    let manifest = fs::read(fixture.manifest()).unwrap();
    let lockfile = fixture.read("Cargo.lock");
    let status = fixture.git(&["status", "--porcelain"]);
    for plan in [None, Some(plan)] {
        let RunOutcome::Check {
            passed,
            message,
            warnings,
        } = run(&RunInput::CheckPublished {
            manifest_path: fixture.manifest(),
            plan,
            verbose: true,
        })
        .unwrap()
        else {
            panic!()
        };
        assert!(passed);
        assert!(!message.is_empty());
        assert!(warnings.is_empty());
    }
    assert_eq!(fs::read(fixture.manifest()).unwrap(), manifest);
    assert_eq!(fixture.read("Cargo.lock"), lockfile);
    assert_eq!(fixture.git(&["status", "--porcelain"]), status);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Starts Cargo with a controlled external checker executable"
)]
fn checker_failures_leave_incomplete_evidence_and_preserve_diagnostics() {
    // Toolchain startup and child-process failures need a last-chance integration watchdog.
    testing::with_watchdog_timeout(Duration::from_mins(5), || {
        let fixture = Fixture::new("");
        write_package(&fixture, "library", "1.0.0", "");
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
                .args(["--base", &fixture.sha("HEAD"), "--output"])
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
                "CARGO_REGISTRY_TOKEN",
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
                assert_ne!(fixture.read("packages/library/src/lib.rs"), original);
                fixture.write("packages/library/src/lib.rs", &original);
            } else {
                assert_eq!(fixture.read("packages/library/src/lib.rs"), original);
            }
        }
    });
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
    serde_json::from_slice(&fs::read(output.join("compatibility.json")).unwrap()).unwrap()
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

/// A Cargo subcommand fixture fails before any real registry query can occur.
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

fn main() {
    let args: Vec<_> = env::args().collect();
    if env::var_os("CRP_FIXTURE_PROBE").is_some() {
        println!("cargo-semver-checks 0.50.0 (fixture)");
        return;
    }
    for name in ["GH_TOKEN", "GITHUB_TOKEN", "CARGO_REGISTRY_TOKEN", "ACTIONS_ID_TOKEN_REQUEST_TOKEN"] {
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
    assert_eq!(Path::new(&manifest).parent().unwrap(), Path::new(&baseline));
    assert!(Path::new(&manifest).is_file());
    assert!(Path::new(&baseline).join("lib.rs").is_file());
    assert!(args.iter().any(|arg| arg == "--all-features"));
    writeln!(calls, "canary").unwrap();
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
    let result = Command::new("cargo")
        .args(["semver-checks", "--version"])
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
