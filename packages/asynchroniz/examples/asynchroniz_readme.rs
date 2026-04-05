//! Example code that matches the `README.md` file.

use asynchroniz::{Mutex, Semaphore};

#[tokio::main]
async fn main() {
    async_mutex().await;
    async_semaphore().await;
}

/// A [`Mutex`] provides async mutual exclusion with a guard that gives
/// `Deref`/`DerefMut` access to the protected value.
async fn async_mutex() {
    let mutex = Mutex::boxed(0_u32);
    let writer = mutex.clone();

    // A background task acquires the lock and increments the value.
    let handle = tokio::spawn(async move {
        let mut guard = writer.lock().await;
        *guard = guard.wrapping_add(1);
    });

    // The main task also increments the value while holding the lock.
    {
        let mut guard = mutex.lock().await;
        *guard = guard.wrapping_add(1);
    }

    // Wait for the background task to complete.
    handle.await.unwrap();

    // Both increments happened under mutual exclusion — the final
    // value is always 2, regardless of which task ran first.
    let guard = mutex.lock().await;
    assert_eq!(*guard, 2);

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
