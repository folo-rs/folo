//! Demonstrates the single-threaded `Local` event variants on a
//! current-thread Tokio runtime.
//!
//! `Local` events avoid locking and atomic operations, making them more
//! efficient when all tasks run on the same thread. They are `!Send`,
//! so they cannot be moved across threads.

use events::{LocalAutoResetEvent, LocalManualResetEvent};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            local_auto_reset().await;
            local_manual_reset().await;
        })
        .await;
}

/// A [`LocalAutoResetEvent`] works like [`AutoResetEvent`] but is
/// restricted to a single thread.
///
/// [`AutoResetEvent`]: events::AutoResetEvent
async fn local_auto_reset() {
    let event = LocalAutoResetEvent::boxed();
    let setter = event.clone();

    // spawn_local requires a LocalSet on the current-thread runtime,
    // which is required for !Send types like LocalAutoResetEvent.
    let worker = tokio::task::spawn_local(async move {
        setter.set();
    });

    event.wait().await;
    assert!(!event.try_wait());
    worker.await.expect("task did not panic");

    println!("LocalAutoResetEvent: signal consumed on single thread.");
}

/// A [`LocalManualResetEvent`] works like [`ManualResetEvent`] but is
/// restricted to a single thread.
///
/// [`ManualResetEvent`]: events::ManualResetEvent
async fn local_manual_reset() {
    let event = LocalManualResetEvent::boxed();
    let setter = event.clone();

    let worker = tokio::task::spawn_local(async move {
        setter.set();
    });

    event.wait().await;

    // Gate stays open.
    assert!(event.try_wait());
    event.wait().await;

    // Reset closes the gate.
    event.reset();
    assert!(!event.try_wait());

    worker.await.expect("task did not panic");

    println!("LocalManualResetEvent: gate opened, verified, and reset.");
}
