//! Example for README.md demonstration of basic `awaiter_set` usage.

use std::pin::pin;
use std::sync::Mutex;
use std::task::Waker;

use awaiter_set::{Awaiter, AwaiterSet};

fn main() {
    // AwaiterSet has no internal synchronization. Protect it with a
    // Mutex (or confine it to a single thread).
    let set = Mutex::new(AwaiterSet::new());

    // Pin an awaiter so it has a stable address while registered.
    let mut awaiter = pin!(Awaiter::new());

    // The primitive registers an awaiter when a future cannot complete
    // immediately, passing a waker to invoke once the resource is available.
    {
        let mut guard = set.lock().unwrap();
        // SAFETY: We hold the lock that protects the set, and the
        // awaiter remains pinned and valid until removed.
        unsafe {
            guard.register(awaiter.as_mut(), Waker::noop().clone());
        }
    }

    // Later, the primitive grants its resource and wakes one awaiter.
    let waker = {
        let mut guard = set.lock().unwrap();
        // SAFETY: We hold the lock that protects the set.
        unsafe { guard.notify_one() }
    };

    // Wake outside the lock to avoid reentrancy with user-defined wakers.
    if let Some(w) = waker {
        w.wake();
    }

    // The future's `poll()` consumes the notification and completes.
    assert!(awaiter.as_ref().take_notification());

    println!("awaiter notified and consumed.");
}
