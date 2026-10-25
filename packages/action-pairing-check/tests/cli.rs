//! Exercises the boolean command contract with local evidence, never GitHub or publication.

use std::fs;
use std::process::Command;

use tempfile::TempDir;
use testing::with_watchdog;

#[test]
#[cfg_attr(miri, ignore = "Native filesystem and subprocess integration.")]
fn unrelated_release_returns_only_false() {
    with_watchdog(|| {
        let directory = TempDir::new().unwrap();
        let report = directory.path().join("report.json");
        let manifest = directory.path().join("release.json");
        fs::write(
            &report,
            r#"{"schema_version":8,"packages":[{"name":"unrelated","status":"pending-release"}]}"#,
        )
        .unwrap();
        fs::write(
            &manifest,
            r#"{"schema_version":1,"tools":[{"name":"tool"}]}"#,
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_action-pairing-check"))
            .args(["--action", "cargo-bench-history"])
            .args(["--report"])
            .arg(&report)
            .arg("--action-manifest")
            .arg(&manifest)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"false\n");
        assert!(output.stderr.is_empty());
    });
}

#[test]
#[cfg_attr(miri, ignore = "Native filesystem and subprocess integration.")]
fn empty_release_does_not_require_a_manifest() {
    with_watchdog(|| {
        let directory = TempDir::new().unwrap();
        let report = directory.path().join("report.json");
        fs::write(&report, r#"{"schema_version":8,"packages":[]}"#).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_action-pairing-check"))
            .args(["--action", "cargo-release-plan"])
            .arg("--report")
            .arg(&report)
            .env("PATH", "")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout, b"false\n");
        assert!(output.stderr.is_empty());
    });
}

#[test]
#[cfg_attr(miri, ignore = "Native filesystem and subprocess integration.")]
fn a_pinned_release_returns_true_and_malformed_input_is_an_error() {
    with_watchdog(|| {
        let directory = TempDir::new().unwrap();
        let report = directory.path().join("report.json");
        let manifest = directory.path().join("proposed action manifest.json");
        fs::write(
            &report,
            r#"{"schema_version":8,"packages":[{"name":"new-tool","status":"pending-release"}]}"#,
        )
        .unwrap();
        fs::write(
            &manifest,
            r#"{"schema_version":1,"tools":[{"name":"new-tool","role":"fixture"}]}"#,
        )
        .unwrap();
        let invoke = |verbose| {
            let mut command = Command::new(env!("CARGO_BIN_EXE_action-pairing-check"));
            if verbose {
                command.arg("--verbose");
            }
            command
                .args(["--action", "cargo-bench-history"])
                .arg("--report")
                .arg(&report)
                .arg("--action-manifest")
                .arg(&manifest)
                .output()
                .unwrap()
        };
        let output = invoke(true);
        assert!(output.status.success());
        assert_eq!(output.stdout, b"true\n");
        assert!(!output.stderr.is_empty());
        fs::write(&manifest, "{}").unwrap();
        let output = invoke(false);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    });
}

#[test]
#[cfg_attr(miri, ignore = "Native filesystem and subprocess integration.")]
fn release_plan_pins_require_pairing_even_when_benchmark_pins_do_not() {
    with_watchdog(|| {
        let directory = TempDir::new().unwrap();
        let report = directory.path().join("report.json");
        let manifest = directory.path().join("release.json");
        fs::write(
            &report,
            r#"{"schema_version":8,"packages":[{"name":"cargo-release-plan","status":"pending-release"}]}"#,
        )
        .unwrap();

        for (action, contents, expected) in [
            (
                "cargo-bench-history",
                r#"{"schema_version":1,"tools":[{"name":"cargo-bench-history"}]}"#,
                b"false\n".as_slice(),
            ),
            (
                "cargo-release-plan",
                r#"{"schema_version":1,"tools":{"cargo-release-plan":{"version":"1.2.3"}}}"#,
                b"true\n".as_slice(),
            ),
        ] {
            fs::write(&manifest, contents).unwrap();
            let output = Command::new(env!("CARGO_BIN_EXE_action-pairing-check"))
                .args(["--action", action, "--report"])
                .arg(&report)
                .arg("--action-manifest")
                .arg(&manifest)
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(output.stdout, expected);
            assert!(output.stderr.is_empty());
        }

        for action in [None, Some("unknown-action")] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_action-pairing-check"));
            command.arg("--report").arg(&report);
            if let Some(action) = action {
                command.args(["--action", action]);
            }
            let output = command.output().unwrap();
            assert!(!output.status.success());
            assert!(output.stdout.is_empty());
            assert!(!output.stderr.is_empty());
        }
    });
}

#[test]
#[cfg_attr(miri, ignore = "Native subprocess integration.")]
fn missing_required_argument_is_an_error() {
    with_watchdog(|| {
        let output = Command::new(env!("CARGO_BIN_EXE_action-pairing-check"))
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    });
}

::testing::set_allocator!();
