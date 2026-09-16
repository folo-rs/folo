//! Integration tests for the machine-readable JSON output written on drop.
//!
//! These exercise the public behavior end to end: dropping a [`Session`] writes
//! one JSON file per operation into the Cargo target directory unless that output
//! is suppressed.

use std::fs::{self, read_to_string, remove_file};
use std::hint::black_box;
use std::io;
use std::path::{Path, PathBuf};

use all_the_time::Session;
use serde_json::Value;
use testing::assert_panics;

// Arbitrary completed batch; tests assert iteration accounting, not elapsed CPU time.
const ITERATIONS: u64 = 100;

/// Resolves the JSON output path that dropping a session writes for `operation`.
fn output_path(operation: &str) -> PathBuf {
    folo_utils::cargo_target_directory()
        .unwrap_or_else(|| "target".into())
        .join("all_the_time")
        .join(format!("{operation}.json"))
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&read_to_string(path).unwrap()).unwrap()
}

/// Removes `path` if it exists, leaving a clean slate for an assertion.
fn remove_if_present(path: &Path) {
    // Attempt the removal unconditionally and tolerate a missing file rather than
    // checking `exists()` first, which would race with parallel test processes.
    match remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => panic!("failed to remove {}: {error}", path.display()),
    }
}

/// Records a fixed amount of processor-time work under `operation` in `session`.
fn record_work(session: &Session, operation: &str) {
    let operation = session.operation(operation);
    let _span = operation.measure_thread().iterations(ITERATIONS);
    for _ in 0..ITERATIONS {
        black_box(black_box(21_u64).wrapping_mul(black_box(2)));
    }
}

#[test]
#[cfg_attr(miri, ignore)] // Uses the real platform and the filesystem, neither supported under Miri.
fn dropping_session_writes_json_into_cargo_target_directory() {
    // A distinctive name avoids colliding with output from other operations in
    // the shared Cargo target directory.
    const OPERATION: &str = "all_the_time_drop_writes_probe";

    let expected = output_path(OPERATION);
    // Start from a clean slate so the assertion proves the drop wrote the file.
    remove_if_present(&expected);

    {
        // Both stdout and file output are enabled; the harness captures the
        // printed summary while the assertion below checks the JSON file.
        let session = Session::new();
        record_work(&session, OPERATION);
    }

    assert!(
        expected.exists(),
        "dropping the session should create {}",
        expected.display()
    );

    let value = read_json(&expected);
    assert_eq!(
        value.get("operation").and_then(Value::as_str),
        Some(OPERATION)
    );
    assert_eq!(
        value.get("total_iterations").and_then(Value::as_u64),
        Some(ITERATIONS)
    );
    assert!(
        value
            .get("total_processor_time_nanos")
            .and_then(Value::as_u64)
            .is_some()
    );

    // Avoid polluting the shared target directory for later runs.
    remove_file(&expected).unwrap();
}

#[test]
#[cfg_attr(miri, ignore)] // Uses the real platform and the filesystem, neither supported under Miri.
fn no_stdout_session_still_writes_json() {
    const OPERATION: &str = "all_the_time_no_stdout_writes_probe";

    let expected = output_path(OPERATION);
    remove_if_present(&expected);

    {
        // Suppressing stdout must not suppress the JSON file output.
        let session = Session::new().no_stdout();
        record_work(&session, OPERATION);
    }

    assert!(
        expected.exists(),
        "no_stdout() should still write {}",
        expected.display()
    );

    let value = read_json(&expected);
    assert_eq!(
        value.get("operation").and_then(Value::as_str),
        Some(OPERATION)
    );

    // Avoid polluting the shared target directory for later runs.
    remove_file(&expected).unwrap();
}

#[test]
#[cfg_attr(miri, ignore)] // Uses the real platform and the filesystem, neither supported under Miri.
fn no_file_suppresses_json_output() {
    const OPERATION: &str = "all_the_time_no_file_probe";

    let expected = output_path(OPERATION);
    remove_if_present(&expected);

    {
        let session = Session::new().no_stdout().no_file();
        record_work(&session, OPERATION);
    }

    assert!(
        !expected.exists(),
        "no_file() should suppress writing {}",
        expected.display()
    );
}

#[test]
#[cfg_attr(miri, ignore)] // Uses the real platform and the filesystem, neither supported under Miri.
fn empty_session_writes_nothing_on_drop() {
    const OPERATION: &str = "all_the_time_empty_drop_probe";

    let expected = output_path(OPERATION);
    remove_if_present(&expected);

    {
        // File output stays enabled, but the session records no measurable work,
        // so dropping it must still write nothing.
        let session = Session::new().no_stdout();
        let _operation = session.operation(OPERATION);
    }

    assert!(
        !expected.exists(),
        "an empty session should not write {}",
        expected.display()
    );
}

#[test]
#[cfg_attr(miri, ignore)] // Uses the real platform and the filesystem, neither supported under Miri.
fn panicking_thread_does_not_write_json() {
    const OPERATION: &str = "all_the_time_panic_guard_probe";

    let expected = output_path(OPERATION);
    remove_if_present(&expected);

    // The session is dropped while the thread unwinds from the panic, so its
    // output must be suppressed to avoid masking the original failure.
    assert_panics(|| {
        let session = Session::new().no_stdout();
        record_work(&session, OPERATION);
        panic!("intentional panic to exercise the drop guard");
    });
    assert!(
        !expected.exists(),
        "a panicking thread should not write {}",
        expected.display()
    );
}

#[test]
#[cfg_attr(miri, ignore = "requires the filesystem and real platform")]
fn directory_output_creates_files_and_overwrites_existing_results() {
    let session = Session::new().no_stdout().no_file();
    record_work(&session, "group/case name");
    record_work(&session, "second");
    _ = session.operation("unmeasured");
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("nested");

    session.to_report().write_to_directory(&target);
    let file = target.join("group_case_name.json");
    assert_eq!(
        read_json(&file).get("operation").unwrap(),
        "group/case name"
    );
    assert_eq!(
        read_json(&target.join("second.json"))
            .get("operation")
            .unwrap(),
        "second"
    );
    assert_eq!(fs::read_dir(&target).unwrap().count(), 2);

    fs::write(&file, "stale contents").unwrap();
    session.to_report().write_to_directory(&target);
    assert_eq!(
        read_json(&file).get("total_iterations").unwrap(),
        ITERATIONS
    );
}

#[test]
#[cfg_attr(miri, ignore = "requires the filesystem")]
fn unmeasured_report_does_not_create_directory() {
    let session = Session::new().no_stdout().no_file();
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("nested");
    session.to_report().write_to_directory(&target);
    assert!(!target.exists());
    _ = session.operation("unmeasured");
    session.to_report().write_to_directory(&target);
    assert!(!target.exists());
}

#[test]
#[cfg_attr(miri, ignore = "requires the filesystem and real platform")]
fn directory_output_surfaces_creation_and_write_errors() {
    let session = Session::new().no_stdout().no_file();
    record_work(&session, "measured");
    let directory = tempfile::tempdir().unwrap();
    let blocker = directory.path().join("blocker");
    fs::write(&blocker, "not a directory").unwrap();
    assert_panics(|| {
        session
            .to_report()
            .write_to_directory(blocker.join("nested"));
    });

    fs::create_dir_all(directory.path().join("measured.json")).unwrap();
    assert_panics(|| session.to_report().write_to_directory(directory.path()));
}

#[test]
#[cfg_attr(miri, ignore = "requires the filesystem and real platform")]
fn colliding_names_fail_before_creating_output_directory() {
    let session = Session::new().no_stdout().no_file();
    for name in ["group/case", "group_case"] {
        record_work(&session, name);
    }
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("nested");
    assert_panics(|| session.to_report().write_to_directory(&target));
    assert!(!target.exists());
}
