# Events

Async manual-reset and auto-reset events for multi-use signaling.

This crate provides two families of event primitives:

- **Manual-reset events** (`ManualResetEvent`, `LocalManualResetEvent`) — a gate that, once set, releases all current and future awaiters until explicitly reset.
- **Auto-reset events** (`AutoResetEvent`, `LocalAutoResetEvent`) — a token dispenser that releases exactly one awaiter per `set()` call.

Each family comes in a thread-safe variant (`Send + Sync`) and a single-threaded `Local` variant for improved efficiency when thread safety is not required.

Events are lightweight cloneable handles. All clones from the same family share the same underlying state.

## Example

```rust
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
    assert!(!event.try_wait());

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
    assert!(event.try_wait());

    println!("ManualResetEvent: gate opened, all waiters released.");
}
```

## See also

More details in the [package documentation](https://docs.rs/events/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for high-performance hardware-aware programming in Rust.