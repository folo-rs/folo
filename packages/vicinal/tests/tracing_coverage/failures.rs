//! Failure diagnostics reached through results, captures and retained schedulers.

use std::panic;
use std::sync::mpsc;

use futures::executor::block_on;
use new_zealand::nz;
use testing::{TracingCapture, assert_panics, assert_panics_with};
use vicinal::WorkerPolicy;

use crate::fixtures::{OnDrop, PanicOnDrop, PanickingPayload, pool};

/// A disconnected result's destructor fails outside the user-task unwind boundary.
pub(crate) fn worker_failure(capture: &TracingCapture) {
    let first_entry = capture.snapshot().len();
    let pool = pool(WorkerPolicy::fixed_per_processor(nz!(1)), "worker-failure");
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
        entry.contains("task cleanup failed") && entry.contains("rejected capture canary")
    }));
    assert!(entries.iter().any(|entry| {
        entry.contains("failure during worker pool cleanup")
            && entry.contains("result disposal canary")
    }));
}

/// Queued destructor failures during an outer unwind must be logged without replacing it.
pub(crate) fn shutdown_during_unwind(capture: &TracingCapture) {
    let first_entry = capture.snapshot().len();
    let pool = pool(
        WorkerPolicy::fixed_per_processor(nz!(1)),
        "shutdown-failure",
    );
    let scheduler = pool.scheduler();
    let (started, entered) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let running = scheduler.spawn(move || {
        started.send(()).unwrap();
        wait.recv().unwrap();
    });
    entered.recv().unwrap();

    let first = PanicOnDrop("queued capture canary");
    scheduler.spawn_urgent_and_forget(move || drop(first));
    let second = PanickingPayload;
    scheduler.spawn_urgent_and_forget(move || drop(second));
    // Shutdown drains queued captures before joining the blocked worker. Tying release
    // to that cleanup keeps the failing tasks queued without timing or private hooks.
    let release = OnDrop(Some(move || release.send(()).unwrap()));
    scheduler.spawn_and_forget(move || drop(release));

    assert_panics_with(
        move || {
            let _pool = pool;
            panic::resume_unwind(Box::new("outer unwind canary"));
        },
        |message| assert!(message.contains("outer unwind canary")),
    );
    block_on(running);

    let snapshot = capture.snapshot();
    let entries = snapshot.get(first_entry..).unwrap();
    assert!(entries.iter().any(|entry| {
        entry.contains("failure during worker pool cleanup")
            && entry.contains("queued capture canary")
    }));
    assert!(
        entries
            .iter()
            .any(|entry| entry.contains("additional panic"))
    );
    assert!(entries.iter().any(|entry| {
        entry.contains("cleanup panicked during unwinding") && entry.contains("pool_id=")
    }));
    assert!(
        entries
            .iter()
            .any(|entry| entry.contains("payload destruction failed"))
    );
}
