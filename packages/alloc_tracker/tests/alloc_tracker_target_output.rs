//! Integration tests for writing machine-readable JSON output to disk.

use std::hint::black_box;

use alloc_tracker::{Allocator, Session};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

#[test]
#[cfg_attr(miri, ignore)] // Uses the global allocator and the filesystem, neither supported under Miri.
fn writes_json_files_for_each_operation() {
    const BYTES_PER_ITERATION: usize = 128;
    const ITERATIONS: u64 = 16;

    let session = Session::new();

    {
        let operation = session.operation("allocate_buffer");
        let _span = operation.measure_thread().iterations(ITERATIONS);
        for _ in 0..ITERATIONS {
            let data: Vec<u8> = black_box(vec![0_u8; BYTES_PER_ITERATION]);
            black_box(&data);
        }
    }

    let directory = tempfile::tempdir().unwrap();
    session.write_to_directory(directory.path()).unwrap();

    let file = directory.path().join("allocate_buffer.json");
    assert!(file.exists(), "expected JSON file for the operation");

    let contents = std::fs::read_to_string(&file).unwrap();
    assert!(contents.contains("\"operation\": \"allocate_buffer\""));
    assert!(contents.contains("\"total_iterations\": 16"));
    assert!(contents.contains("\"total_bytes_allocated\""));
    assert!(contents.contains("\"total_allocations_count\""));
    assert!(contents.contains("\"mean_bytes_per_iteration\""));
    assert!(contents.contains("\"mean_allocations_per_iteration\""));
}

#[test]
#[cfg_attr(miri, ignore)] // Uses the global allocator and the filesystem, neither supported under Miri.
fn empty_session_writes_nothing() {
    let session = Session::new();
    let _operation = session.operation("never_measured");

    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("output");

    session.write_to_directory(&target).unwrap();

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
        .unwrap_or_else(|| std::path::PathBuf::from("target"))
        .join("alloc_tracker")
        .join(format!("{OPERATION}.json"));

    // Start from a clean slate so the assertion proves this call wrote the file.
    if expected.exists() {
        std::fs::remove_file(&expected).unwrap();
    }

    session.write_to_target().unwrap();

    assert!(
        expected.exists(),
        "write_to_target should create {}",
        expected.display()
    );

    let contents = std::fs::read_to_string(&expected).unwrap();
    assert!(contents.contains("\"operation\": \"alloc_tracker_write_to_target_probe\""));
    assert!(contents.contains("\"total_iterations\": 8"));

    // Avoid polluting the shared target directory for later runs.
    std::fs::remove_file(&expected).unwrap();
}
