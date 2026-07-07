//! Thread safety integration tests for `all_the_time`.
//!
//! These tests verify that the public API types can be safely moved
//! between threads and that thread-safety changes work correctly.

use std::thread;

use all_the_time::{Report, Session};

#[test]
#[cfg_attr(miri, ignore)] // The real platform functionality cannot be accessed under Miri.
fn session_can_be_moved_between_threads() {
    let session = Session::new().no_stdout().no_file();

    // Move session to another thread
    let handle = thread::spawn(move || {
        let operation = session.operation("cross_thread_work");
        let _span = operation.measure_thread().iterations(1);

        // Do some work
        let mut sum = 0;
        for i in 0..1000 {
            sum += i;
        }
        std::hint::black_box(sum);

        session.to_report()
    });

    let report = handle.join().unwrap();
    // The important thing is that the session was successfully moved between threads
    // The report may or may not be empty depending on timing, but it should exist
    let _has_operations = report.operations().count();
}

#[test]
#[cfg_attr(miri, ignore)] // The real platform functionality cannot be accessed under Miri.
fn operation_can_be_moved_between_threads() {
    let session = Session::new().no_stdout().no_file();
    let operation = session.operation("test_op");

    // Move operation to another thread
    let handle = thread::spawn(move || {
        let _span = operation.measure_process().iterations(1);

        // Do some work
        let mut sum = 0;
        for i in 0..1000 {
            sum += i;
        }
        std::hint::black_box(sum);
    });

    handle.join().unwrap();

    // Inspect the recorded measurement the way a user would: via a report.
    let report = session.to_report();
    let processor_time = report
        .operations()
        .find(|&(name, _)| name == "test_op")
        .and_then(|(_, op)| op.processor_time());
    // A `Duration` is always non-negative; asserting `Some` confirms the report
    // exposes an estimable per-iteration figure for the recorded span.
    assert!(processor_time.is_some_and(|t| t >= std::time::Duration::ZERO));
}

#[test]
#[cfg_attr(miri, ignore)] // The real platform functionality cannot be accessed under Miri.
fn report_can_be_shared_across_threads() {
    let session = Session::new().no_stdout().no_file();
    {
        let operation = session.operation("shared_work");
        let _span = operation.measure_thread().iterations(1);

        // Do some work
        let mut sum = 0;
        for i in 0..1000 {
            sum += i;
        }
        std::hint::black_box(sum);
    }

    let report = session.to_report();

    // Send report to multiple threads
    let report_clone = report.clone();
    let handle1 = thread::spawn(move || !report.is_empty());

    let handle2 = thread::spawn(move || !report_clone.is_empty());

    assert!(handle1.join().unwrap());
    assert!(handle2.join().unwrap());
}

#[test]
#[cfg_attr(miri, ignore)] // The real platform functionality cannot be accessed under Miri.
fn reports_can_be_merged_across_threads() {
    let session1 = Session::new().no_stdout().no_file();
    let session2 = Session::new().no_stdout().no_file();

    // Create reports in different threads
    let handle1 = thread::spawn(move || {
        let operation = session1.operation("thread1_work");
        let _span = operation.measure_thread().iterations(1);

        let mut sum = 0;
        for i in 0..500 {
            sum += i;
        }
        std::hint::black_box(sum);

        session1.to_report()
    });

    let handle2 = thread::spawn(move || {
        let operation = session2.operation("thread2_work");
        let _span = operation.measure_process().iterations(1);

        let mut sum = 0;
        for i in 0..500 {
            sum += i;
        }
        std::hint::black_box(sum);

        session2.to_report()
    });

    let report1 = handle1.join().unwrap();
    let report2 = handle2.join().unwrap();

    // Merge reports from different threads
    let merged = Report::merge(&report1, &report2);
    // The important part is that reports could be moved between threads and merged
    // Reports may be empty but the merge operation should work
    let _operation_count = merged.operations().count();
}
