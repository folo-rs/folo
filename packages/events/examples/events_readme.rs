//! Example for README.md demonstration of basic `events` usage.

use events::{AutoResetEvent, ManualResetEvent};

#[tokio::main]
async fn main() {
    auto_reset_signal().await;
    manual_reset_gate().await;
}

/// An [`AutoResetEvent`] releases exactly one awaiter per `set()` call.
/// If no one is waiting, the signal is remembered for the next waiter.
async fn auto_reset_signal() {
    let event = AutoResetEvent::boxed();
    let setter = event.clone();

    // Producer signals from a background task.
    tokio::spawn(async move {
        setter.set();
    });

    // Consumer waits for the signal.
    event.wait().await;

    // The signal was consumed — a second check returns false.
    assert!(!event.try_acquire());

    println!("AutoResetEvent: signal received and consumed.");
}

/// A [`ManualResetEvent`] acts as a gate: once set, all current and future
/// awaiters pass through until the event is explicitly reset.
async fn manual_reset_gate() {
    let event = ManualResetEvent::boxed();
    let setter = event.clone();

    // Producer opens the gate from a background task.
    tokio::spawn(async move {
        setter.set();
    });

    // Consumer waits for the gate to open.
    event.wait().await;

    // The gate stays open — subsequent waits complete immediately.
    event.wait().await;
    assert!(event.is_set());

    println!("ManualResetEvent: gate opened, all waiters released.");
}
