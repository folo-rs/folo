//! Failure diagnostics reached through results, captures and retained schedulers.

use std::sync::mpsc;

use new_zealand::nz;
use testing::{TracingCapture, assert_panics, assert_panics_with};
use vicinal::policies::WorkersPerProcessor;

use crate::fixtures::{PanicOnDrop, pool};

/// A disconnected result's destructor fails outside the user-task unwind boundary.
pub(crate) fn worker_failure(capture: &TracingCapture) {
    let first_entry = capture.snapshot().len();
    let pool = pool(WorkersPerProcessor::new(nz!(1)), "worker-failure");
    let scheduler = pool.scheduler();
    let (started, entered) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let result = scheduler.spawn(move || {
        started.send(()).unwrap();
        wait.recv().unwrap();
        PanicOnDrop("result disposal canary")
    });
    entered.recv().unwrap();
    drop(result);
    release.send(()).unwrap();
    assert_panics_with(
        || drop(pool),
        |message| assert!(message.contains("result disposal canary")),
    );

    // Poisoned rejection must clean up the new capture behind its own unwind boundary.
    let capture_on_rejection = PanicOnDrop("rejected capture canary");
    assert_panics(|| scheduler.spawn(move || drop(capture_on_rejection)));
    let snapshot = capture.snapshot();
    let entries = snapshot.get(first_entry..).unwrap();
    assert!(entries.iter().any(|entry| {
        entry.contains("infrastructure failed")
            && entry.contains("result disposal canary")
            && entry.contains("pool_id=")
    }));
    assert!(entries.iter().any(|entry| {
        entry.contains("task cleanup failed")
            && entry.contains("vicinal::task:")
            && entry.contains("rejected capture canary")
    }));
    assert!(entries.iter().any(|entry| {
        entry.contains("failure during worker pool cleanup")
            && entry.contains("result disposal canary")
    }));
}
