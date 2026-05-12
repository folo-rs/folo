# awaiter_set

Zero-allocation awaiter tracking for async synchronization primitives.

Async synchronization primitives (such as events) need a way to track which futures are waiting
and wake them when a resource becomes available. This crate provides that mechanism with two
types:

- `AwaiterSet` — the set of registered awaiters, owned by the synchronization primitive.
- `Awaiter` — a single awaiter, embedded inside an awaiting future.

Each awaiter lives directly inside its future rather than being heap-allocated, making
registration and removal zero-allocation. Awaiters must remain at a stable pinned address from
registration until removal.

Neither `AwaiterSet` nor `Awaiter` has internal synchronization, and both are `Send` but not
`Sync`. To share an `AwaiterSet` between threads, wrap it in a `Mutex` (or another exclusive
lock); for single-threaded use no lock is required. The mutating methods take `&mut self`, so
the borrow checker (or a wrapping mutex) already enforces exclusive access. `Awaiter`'s public
methods are safe to call from the owning future at any time.

## Example

```rust
use std::pin::pin;
use std::sync::Mutex;
use std::task::Waker;

use awaiter_set::{Awaiter, AwaiterSet};

fn main() {
    // AwaiterSet is !Sync. To share between threads, wrap it in a
    // Mutex; for single-threaded use no lock is required.
    let set = Mutex::new(AwaiterSet::new());

    // Pin an awaiter so it has a stable address while registered.
    let mut awaiter = pin!(Awaiter::new());

    // The primitive registers an awaiter when a future cannot complete
    // immediately, passing a waker to invoke once the resource is available.
    {
        let mut guard = set.lock().unwrap();
        // SAFETY: The awaiter is stack-pinned and is unregistered (via
        // take_notification clearing it after notify_one) before being dropped.
        unsafe {
            guard.register(awaiter.as_mut(), Waker::noop().clone());
        }
    }

    // Later, the primitive grants its resource and wakes one awaiter.
    let waker = {
        let mut guard = set.lock().unwrap();
        // SAFETY: The only registered awaiter is stack-pinned and outlives this call.
        unsafe { guard.notify_one() }
    };

    // Wake outside the lock to avoid reentrancy with user-defined wakers.
    if let Some(w) = waker {
        w.wake();
    }

    // The future's `poll()` consumes the notification and completes.
    assert!(awaiter.take_notification());

    println!("awaiter notified and consumed.");
}
```

## See also

More details in the [package documentation](https://docs.rs/awaiter_set/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.
