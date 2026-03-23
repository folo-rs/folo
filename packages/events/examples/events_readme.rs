//! Example for README.md demonstration of basic `events` usage.

use events::{AutoResetEvent, ManualResetEvent, Mutex, Semaphore};

#[tokio::main]
async fn main() {
    auto_reset_signal().await;
    manual_reset_gate().await;
    async_mutex().await;
    async_semaphore().await;
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

/// A [`Mutex`] provides async mutual exclusion with a guard that gives
/// `Deref`/`DerefMut` access to the protected value.
async fn async_mutex() {
    let mutex = Mutex::boxed(0_u32);
    let writer = mutex.clone();

    // A background task acquires the lock and mutates the value.
    tokio::spawn(async move {
        let mut guard = writer.lock().await;
        *guard = 42;
    });

    // The main task waits for the lock and reads the value.
    let guard = mutex.lock().await;
    assert_eq!(*guard, 42);

    println!("Mutex: protected value is {}", *guard);
}

/// A [`Semaphore`] controls concurrent access by maintaining a pool of
/// permits. Each `acquire()` consumes a permit and each drop returns it.
async fn async_semaphore() {
    let sem = Semaphore::boxed(2);

    // Acquire two permits, exhausting the pool.
    let permit_a = sem.acquire().await;
    let permit_b = sem.acquire().await;

    // No permits left — try_acquire returns None.
    assert!(sem.try_acquire().is_none());

    // Releasing one permit makes it available again.
    drop(permit_a);
    let _permit_c = sem.acquire().await;

    drop(permit_b);

    println!("Semaphore: permits acquired and released.");
}
