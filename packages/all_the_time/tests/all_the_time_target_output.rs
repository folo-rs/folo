//! Integration tests for writing machine-readable JSON output to disk.

use std::fs;
use std::hint::black_box;

use all_the_time::Session;

#[test]
#[cfg_attr(miri, ignore)] // Uses the real platform and the filesystem, neither supported under Miri.
fn writes_json_files_for_each_operation() {
    let session = Session::new();

    {
        let operation = session.operation("first_operation");
        let _span = operation.measure_thread().iterations(100);
        for _ in 0..100 {
            black_box(black_box(21_u64) * black_box(2));
        }
    }

    {
        let operation = session.operation("second_operation");
        let _span = operation.measure_thread().iterations(50);
        for _ in 0..50 {
            black_box(black_box(3_u64) + black_box(4));
        }
    }

    let directory = tempfile::tempdir().unwrap();
    session.write_to_directory(directory.path());

    let first = directory.path().join("first_operation.json");
    let second = directory.path().join("second_operation.json");

    assert!(first.exists(), "expected JSON file for first operation");
    assert!(second.exists(), "expected JSON file for second operation");

    // Exactly one file per measured operation, and nothing more.
    let written = fs::read_dir(directory.path()).unwrap().count();
    assert_eq!(written, 2, "expected one JSON file per operation");

    let first_contents = fs::read_to_string(&first).unwrap();
    assert!(first_contents.contains("\"operation\": \"first_operation\""));
    assert!(first_contents.contains("\"total_iterations\": 100"));
    assert!(first_contents.contains("\"total_processor_time_nanos\""));
    assert!(first_contents.contains("\"mean_processor_time_nanos\""));
}

#[test]
#[cfg_attr(miri, ignore)] // Uses the real platform and the filesystem, neither supported under Miri.
fn report_and_session_write_equivalently() {
    let session = Session::new();

    {
        let operation = session.operation("shared_name");
        let _span = operation.measure_thread().iterations(10);
        for _ in 0..10 {
            black_box(black_box(1_u64) + black_box(1));
        }
    }

    let from_session = tempfile::tempdir().unwrap();
    let from_report = tempfile::tempdir().unwrap();

    session.write_to_directory(from_session.path());
    session.to_report().write_to_directory(from_report.path());

    let session_file = from_session.path().join("shared_name.json");
    let report_file = from_report.path().join("shared_name.json");

    assert!(session_file.exists());
    assert!(report_file.exists());

    // Both entry points capture the same data (iteration count is deterministic).
    let session_contents = fs::read_to_string(&session_file).unwrap();
    let report_contents = fs::read_to_string(&report_file).unwrap();
    assert!(session_contents.contains("\"total_iterations\": 10"));
    assert!(report_contents.contains("\"total_iterations\": 10"));
}

#[test]
#[cfg_attr(miri, ignore)] // Uses the real platform and the filesystem, neither supported under Miri.
fn empty_session_writes_nothing() {
    let session = Session::new();
    let _operation = session.operation("never_measured");

    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("output");

    session.write_to_directory(&target);

    assert!(!target.exists(), "no directory should be created");
}

#[test]
#[cfg_attr(miri, ignore)] // Resolves the Cargo target directory and writes files, neither supported under Miri.
fn write_to_target_writes_into_cargo_target_directory() {
    // A distinctive name avoids colliding with output from other operations in
    // the shared Cargo target directory.
    const OPERATION: &str = "all_the_time_write_to_target_probe";

    let session = Session::new();
    {
        let operation = session.operation(OPERATION);
        let _span = operation.measure_thread().iterations(8);
        for _ in 0..8 {
            black_box(black_box(2_u64) * black_box(3));
        }
    }

    let expected = folo_utils::cargo_target_directory()
        .unwrap_or_else(|| "target".into())
        .join("all_the_time")
        .join(format!("{OPERATION}.json"));

    // Start from a clean slate so the assertion proves this call wrote the file.
    if expected.exists() {
        fs::remove_file(&expected).unwrap();
    }

    session.write_to_target();

    assert!(
        expected.exists(),
        "write_to_target should create {}",
        expected.display()
    );

    let contents = fs::read_to_string(&expected).unwrap();
    assert!(contents.contains("\"operation\": \"all_the_time_write_to_target_probe\""));
    assert!(contents.contains("\"total_iterations\": 8"));

    // Avoid polluting the shared target directory for later runs.
    fs::remove_file(&expected).unwrap();
}
