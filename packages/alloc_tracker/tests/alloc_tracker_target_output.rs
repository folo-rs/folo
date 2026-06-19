//! Integration tests for writing machine-readable JSON output to disk.

use std::fs;
use std::hint::black_box;
use std::path::Path;

use alloc_tracker::{Allocator, Session};
use serde_json::Value;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
#[cfg_attr(miri, ignore)] // Uses the global allocator and the filesystem, neither supported under Miri.
fn writes_json_files_for_each_operation() {
    const FIRST_BYTES_PER_ITERATION: usize = 128;
    const FIRST_ITERATIONS: u64 = 16;
    const SECOND_BYTES_PER_ITERATION: usize = 64;
    const SECOND_ITERATIONS: u64 = 8;

    let session = Session::new();

    {
        let operation = session.operation("allocate_buffer");
        let _span = operation.measure_thread().iterations(FIRST_ITERATIONS);
        for _ in 0..FIRST_ITERATIONS {
            let data: Vec<u8> = black_box(vec![0_u8; FIRST_BYTES_PER_ITERATION]);
            black_box(&data);
        }
    }

    {
        let operation = session.operation("allocate_small_buffer");
        let _span = operation.measure_thread().iterations(SECOND_ITERATIONS);
        for _ in 0..SECOND_ITERATIONS {
            let data: Vec<u8> = black_box(vec![0_u8; SECOND_BYTES_PER_ITERATION]);
            black_box(&data);
        }
    }

    let directory = tempfile::tempdir().unwrap();
    session.write_to_directory(directory.path());

    let first = directory.path().join("allocate_buffer.json");
    let second = directory.path().join("allocate_small_buffer.json");

    assert!(first.exists(), "expected JSON file for first operation");
    assert!(second.exists(), "expected JSON file for second operation");

    // Exactly one file per measured operation, and nothing more.
    let written = fs::read_dir(directory.path()).unwrap().count();
    assert_eq!(written, 2, "expected one JSON file per operation");

    let value = read_json(&first);
    assert_eq!(
        value.get("operation").and_then(Value::as_str),
        Some("allocate_buffer")
    );
    assert_eq!(
        value.get("total_iterations").and_then(Value::as_u64),
        Some(16)
    );
    assert!(
        value
            .get("total_bytes_allocated")
            .and_then(Value::as_u64)
            .is_some()
    );
    assert!(
        value
            .get("total_allocations_count")
            .and_then(Value::as_u64)
            .is_some()
    );
    assert!(
        value
            .get("mean_bytes_per_iteration")
            .and_then(Value::as_u64)
            .is_some()
    );
    assert!(
        value
            .get("mean_allocations_per_iteration")
            .and_then(Value::as_u64)
            .is_some()
    );
}

#[test]
#[cfg_attr(miri, ignore)] // Uses the global allocator and the filesystem, neither supported under Miri.
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
    const OPERATION: &str = "alloc_tracker_write_to_target_probe";
    const BYTES_PER_ITERATION: usize = 64;
    const ITERATIONS: u64 = 8;

    let session = Session::new();
    {
        let operation = session.operation(OPERATION);
        let _span = operation.measure_thread().iterations(ITERATIONS);
        for _ in 0..ITERATIONS {
            let data: Vec<u8> = black_box(vec![0_u8; BYTES_PER_ITERATION]);
            black_box(&data);
        }
    }

    let expected = folo_utils::cargo_target_directory()
        .unwrap_or_else(|| "target".into())
        .join("alloc_tracker")
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

    let value = read_json(&expected);
    assert_eq!(
        value.get("operation").and_then(Value::as_str),
        Some("alloc_tracker_write_to_target_probe")
    );
    assert_eq!(
        value.get("total_iterations").and_then(Value::as_u64),
        Some(8)
    );

    // Avoid polluting the shared target directory for later runs.
    fs::remove_file(&expected).unwrap();
}

#[test]
#[cfg_attr(miri, ignore)] // Uses the global allocator, which is not supported under Miri.
#[should_panic(expected = "after sanitization")]
fn panics_when_operation_names_collide_after_sanitization() {
    let session = Session::new();

    // Both names sanitize to the same file name, which must not silently
    // overwrite one operation's results.
    for name in ["group/case", "group_case"] {
        let operation = session.operation(name);
        let _span = operation.measure_thread().iterations(4);
        for _ in 0..4 {
            let data: Vec<u8> = black_box(vec![0_u8; 64]);
            black_box(&data);
        }
    }

    // The collision is detected before anything is written, so this path is
    // never created.
    session.write_to_directory("collision_is_detected_before_writing");
}
