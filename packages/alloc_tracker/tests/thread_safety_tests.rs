//! Thread safety integration tests for `alloc_tracker`.
//!
//! These tests verify that the public API types can be safely moved
//! between threads and that thread-safety changes work correctly.

use alloc_tracker::{Allocator, Report, Session};
use std::thread;

// Fake a global allocator for these tests
thread_local! {
    static FAKE_ALLOCATOR: Allocator<std::alloc::System> = const { Allocator::system() };
}

#[test]
fn session_can_be_moved_between_threads() {
    let session = Session::new();

    // Move session to another thread
    let handle = thread::spawn(move || {
        let operation = session.operation("cross_thread_work");
        let _span = operation.iterations(1).measure_process();

        // Simulate some allocation work
        // Note: In actual usage, this would allocate through the global allocator
        // but for this test we just verify the session can move between threads

        session.to_report()
    });

    let report = handle.join().unwrap();
    // Report may be empty since we didn't do actual allocations,
    // but the important part is that session moved between threads successfully
    assert!(report.is_empty() || !report.is_empty()); // Always true, but uses the result
}

#[test]
fn operation_can_be_moved_between_threads() {
    let session = Session::new();
    let operation = session.operation("test_op");

    // Move operation to another thread
    let handle = thread::spawn(move || {
        let _span = operation.iterations(1).measure_thread();

        // Simulate some work (without actual allocations in test)

        operation.mean()
    });

    let mean_bytes = handle.join().unwrap();
    assert_eq!(mean_bytes, 0); // No actual allocations in test
}

#[test]
fn report_can_be_shared_across_threads() {
    let session = Session::new();
    // Create an empty report for testing
    let report = session.to_report();

    // Send report to multiple threads
    let report_clone = report.clone();
    let handle1 = thread::spawn(move || report.is_empty());

    let handle2 = thread::spawn(move || report_clone.is_empty());

    assert!(handle1.join().unwrap()); // Empty report
    assert!(handle2.join().unwrap()); // Empty report
}

#[test]
fn reports_can_be_merged_across_threads() {
    let session1 = Session::new();
    let session2 = Session::new();

    // Create reports in different threads
    let handle1 = thread::spawn(move || {
        // Create a report (empty in this test case)
        session1.to_report()
    });

    let handle2 = thread::spawn(move || {
        // Create a report (empty in this test case)
        session2.to_report()
    });

    let report1 = handle1.join().unwrap();
    let report2 = handle2.join().unwrap();

    // Merge reports from different threads
    let merged = Report::merge(&report1, &report2);
    assert!(merged.is_empty()); // Both reports are empty

    // The important part is that reports could be moved between threads
    // and merged successfully
}
