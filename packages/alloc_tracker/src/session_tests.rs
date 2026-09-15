//! Library coverage of session lifecycle and automatic output. Output tests run this
//! harness's probe in a child process so stdout and Cargo target resolution are isolated
//! without changing the parent environment or replacing production output behavior.

use std::process::Command;
use std::{env, fs, slice};

use serde_json::Value;
use tempfile::tempdir;
use testing::{assert_panics, with_watchdog};

use crate::Session;
use crate::counters::register_fake_allocation;

// Distinctive fixture data separates report output from libtest's own output and
// distinguishes iterations, bytes and allocation counts without a global allocator.
const OPERATION: &str = "session_output_probe_work";
const ITERATIONS: u64 = 7;
const BYTES: u64 = 91;
const ALLOCATIONS: u64 = 13;
const CASE_ENV: &str = "ALLOC_TRACKER_SESSION_OUTPUT_CASE";
const OUTPUT_BEGIN: &str = "alloc-tracker-output-begin\n";
const OUTPUT_END: &str = "alloc-tracker-output-end\n";

fn record(session: &Session, iterations: u64) {
    let operation = session.operation(OPERATION);
    let _span = operation.measure_thread().iterations(iterations);
    register_fake_allocation(BYTES, ALLOCATIONS);
}

#[test]
fn empty_session_lifecycle() {
    let session = Session::new().no_stdout().no_file();
    assert!(session.is_empty());

    _ = session.operation("unmeasured");
    assert!(session.is_empty());

    // Allocator activity alone cannot define a rate when no iterations completed.
    record(&session, 0);
    assert!(session.is_empty());

    record(&session, ITERATIONS);
    assert!(!session.is_empty());

    _ = session.operation("also_unmeasured");
    assert!(!session.is_empty());
}

#[test]
fn completed_iterations_without_allocations_are_not_empty() {
    let session = Session::new().no_stdout().no_file();
    let operation = session.operation(OPERATION);
    drop(operation.measure_thread().iterations(ITERATIONS));

    assert!(!session.is_empty());
}

#[test]
#[cfg_attr(
    miri,
    ignore = "spawns a child harness and observes real stdout and filesystem output"
)]
fn default_drop_emits_both_outputs() {
    assert_output("both", true, true);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "spawns a child harness and observes real stdout and filesystem output"
)]
fn no_stdout_retains_file_output() {
    assert_output("file", false, true);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "spawns a child harness and observes real stdout and filesystem output"
)]
fn no_file_retains_stdout_output() {
    assert_output("stdout", true, false);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "spawns a child harness and observes real stdout and filesystem output"
)]
fn disabled_outputs_leave_no_trace() {
    assert_output("disabled", false, false);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "spawns a child harness and observes real stdout and filesystem output"
)]
fn unused_session_leaves_no_trace() {
    assert_output("unused", false, false);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "spawns a child harness and observes real stdout and filesystem output"
)]
fn unmeasured_operations_leave_no_trace() {
    assert_output("unmeasured", false, false);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "spawns a child harness and observes real stdout and filesystem output"
)]
fn zero_iterations_leave_no_trace() {
    assert_output("zero", false, false);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "spawns a child harness and observes real stdout and filesystem output"
)]
fn unwinding_skips_stdout_and_an_unwritable_target() {
    assert_output("unwind", false, false);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "spawns a child harness and observes real stdout and filesystem output"
)]
fn report_uses_the_cargo_target_destination() {
    assert_output("report", false, true);
}

fn assert_output(case: &'static str, stdout: bool, file: bool) {
    with_watchdog(move || {
        let directory = tempdir().unwrap();
        let target = directory.path().join("selected-target");
        if case == "unwind" {
            // A file in place of the target directory makes any attempted JSON output
            // fail, so skipping output must also protect the original panic.
            fs::write(&target, b"not a directory").unwrap();
        }

        let output = Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                "session_tests::output_probe",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CASE_ENV, case)
            .env("CARGO_TARGET_DIR", &target)
            .current_dir(directory.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");

        let output = String::from_utf8(output.stdout).unwrap();
        let (_, output) = output.split_once(OUTPUT_BEGIN).unwrap();
        let (output, _) = output.split_once(OUTPUT_END).unwrap();
        if stdout {
            assert!(output.contains(OPERATION));
        } else {
            assert!(output.is_empty());
        }

        let output_directory = target.join("alloc_tracker");
        if file {
            let paths = fs::read_dir(&output_directory)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            let expected = output_directory.join(format!("{OPERATION}.json"));
            assert_eq!(paths, slice::from_ref(&expected));

            let value: Value = serde_json::from_slice(&fs::read(expected).unwrap()).unwrap();
            assert_eq!(
                value.get("operation").and_then(Value::as_str),
                Some(OPERATION)
            );
            assert_eq!(
                value.get("total_iterations").and_then(Value::as_u64),
                Some(ITERATIONS)
            );
            assert_eq!(
                value.get("total_bytes_allocated").and_then(Value::as_u64),
                Some(BYTES)
            );
            assert_eq!(
                value.get("total_allocations_count").and_then(Value::as_u64),
                Some(ALLOCATIONS)
            );
        } else if case == "unwind" {
            assert_eq!(fs::read(&target).unwrap(), b"not a directory");
        } else {
            assert!(!target.exists());
        }
        // Resolving the configured target must not leave output in the cwd fallback.
        assert!(!directory.path().join("target").exists());
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "child harness fixture for real stdout and filesystem output"
)]
fn output_probe() {
    let Some(case) = env::var_os(CASE_ENV) else {
        // Ordinary test discovery runs the fixture without selecting a scenario.
        return;
    };
    let case = case.to_str().unwrap();
    let session = match case {
        "both" | "unused" | "unmeasured" | "zero" | "unwind" => Session::new(),
        "file" => Session::new().no_stdout(),
        "stdout" => Session::new().no_file(),
        "disabled" | "report" => Session::new().no_stdout().no_file(),
        _ => panic!("unknown output probe case"),
    };

    match case {
        "unused" => {}
        "unmeasured" => {
            _ = session.operation(OPERATION);
        }
        "zero" => record(&session, 0),
        _ => record(&session, ITERATIONS),
    }

    print!("{OUTPUT_BEGIN}");
    if case == "unwind" {
        assert_panics(|| {
            let _session = session;
            panic!("unwind output probe");
        });
    } else if case == "report" {
        session.to_report().write_to_target();
        drop(session);
    } else {
        drop(session);
    }
    print!("{OUTPUT_END}");
}
