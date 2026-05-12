//! Example for README.md demonstration of basic `awaiter_set` usage.

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
        guard.notify_one()
    };

    // Wake outside the lock to avoid reentrancy with user-defined wakers.
    if let Some(w) = waker {
        w.wake();
    }

    // The future's `poll()` consumes the notification and completes.
    assert!(awaiter.take_notification());

    println!("awaiter notified and consumed.");
}
