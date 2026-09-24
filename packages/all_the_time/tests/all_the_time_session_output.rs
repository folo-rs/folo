//! Captures real session-drop stdout and files in an isolated child test process.
//! The parent owns the temporary Cargo target; no process-global environment is changed.

use std::process::Command;
use std::{env, fs};

use all_the_time::Session;
use serde_json::Value;
use testing::{assert_panics, with_watchdog};

// Distinct from test names so libtest's own progress output cannot satisfy the assertion.
const OPERATION: &str = "drop_emission_probe";
const SCENARIO: &str = "ALL_THE_TIME_OUTPUT_SCENARIO";

#[test]
#[cfg_attr(miri, ignore = "requires child processes and real output")]
fn session_output_respects_destinations_and_lifecycle() {
    with_watchdog(|| {
        for (scenario, stdout, file) in [
            ("both", true, true),
            ("stdout", true, false),
            ("file", false, true),
            ("neither", false, false),
            ("empty", false, false),
            ("unmeasured", false, false),
            ("unwind", false, false),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let output = Command::new(env::current_exe().unwrap())
                .args(["--exact", "output_probe", "--ignored", "--nocapture"])
                .env(SCENARIO, scenario)
                .env("CARGO_TARGET_DIR", directory.path())
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            let text = String::from_utf8(output.stdout).unwrap();
            assert_eq!(text.contains(OPERATION), stdout, "{scenario}");
            let path = directory
                .path()
                .join("all_the_time")
                .join(format!("{OPERATION}.json"));
            assert_eq!(path.exists(), file, "{scenario}");
            if file {
                let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
                assert_eq!(value.get("operation").unwrap(), OPERATION);
                assert_eq!(value.get("total_iterations").unwrap(), 1);
            } else {
                assert!(!directory.path().join("all_the_time").exists());
            }
        }
    });
}

// This fixture is only invoked by the parent above, with isolated output destinations.
#[test]
#[ignore = "child process fixture for session output integration coverage"]
fn output_probe() {
    let scenario = env::var(SCENARIO).unwrap();
    let run = || {
        let session = Session::new();
        let session = if matches!(scenario.as_str(), "file" | "neither") {
            session.no_stdout()
        } else {
            session
        };
        let session = if matches!(scenario.as_str(), "stdout" | "neither") {
            session.no_file()
        } else {
            session
        };
        if scenario != "empty" {
            let operation = session.operation(OPERATION);
            if scenario != "unmeasured" {
                // A completed span is sufficient even if the OS clock does not advance.
                drop(operation.measure_thread().iterations(1));
            }
        }
        assert_ne!(scenario, "unwind", "exercise session drop during unwinding");
    };
    if scenario == "unwind" {
        assert_panics(run);
    } else {
        run();
    }
}

::testing::set_allocator!();
