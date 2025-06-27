//! Basic usage example of the `once_event` crate with the new API.

use std::f64::consts::{E, PI};
use std::future::Future;
use std::task;

use futures::FutureExt;
use futures::task::noop_waker_ref;
use once_event::{OnceEventEmbedded, OnceEventPoolByRc, OnceEventPoolByRef, OnceEventPoolUnsafe};

fn main() {
    // Example 1: Embedded storage for single-use events
    embedded_example();

    // Example 2: Pool storage with reference-based access
    pool_by_ref_example();

    // Example 3: Pool storage with Rc-based access
    pool_by_rc_example();

    // Example 4: Pool storage with unsafe access
    unsafe_pool_example();
}

/// Simple polling function that blocks until the future is ready.
fn poll_to_completion<T>(mut future: impl Future<Output = T> + Unpin) -> T {
    let waker = noop_waker_ref();
    let mut cx = task::Context::from_waker(waker);

    loop {
        match future.poll_unpin(&mut cx) {
            task::Poll::Ready(value) => return value,
            task::Poll::Pending => {
                // In a real async runtime, we would yield here
                // For this example, we just continue polling
                continue;
            }
        }
    }
}

fn embedded_example() {
    println!("=== Embedded Storage Example ===");

    // Create and pin embedded storage
    let mut storage = Box::pin(OnceEventEmbedded::<String>::new());

    // Activate to get sender/receiver pair
    let (sender, receiver) = storage.as_mut().activate();

    // Set a value
    sender.set("Hello from embedded storage!".to_string());

    // Await the result
    let result = poll_to_completion(receiver);
    println!("Received: {result}");

    // Storage becomes inert after use and can be reused
    assert!(storage.is_inert());

    // Activate again for another event
    let (sender2, receiver2) = storage.as_mut().activate();
    sender2.set("Second message!".to_string());
    let result2 = poll_to_completion(receiver2);
    println!("Received: {result2}");

    println!();
}

fn pool_by_ref_example() {
    println!("=== Pool by Reference Example ===");

    // Create a pool that can handle multiple concurrent events
    let pool = OnceEventPoolByRef::<i32>::new();

    // Create multiple events from the same pool
    let (sender1, receiver1) = pool.activate();
    let (sender2, receiver2) = pool.activate();
    let (sender3, receiver3) = pool.activate();

    // Send values
    sender1.set(100);
    sender2.set(200);
    sender3.set(300);

    // Receive values (order doesn't matter)
    let result1 = poll_to_completion(receiver1);
    let result2 = poll_to_completion(receiver2);
    let result3 = poll_to_completion(receiver3);

    println!("Results: {result1}, {result2}, {result3}");
    println!();
}

fn pool_by_rc_example() {
    println!("=== Pool by Rc Example ===");

    // Create a pool wrapped in Rc for easier sharing without lifetimes
    let pool = OnceEventPoolByRc::<f64>::new();

    // Create events
    let (sender1, receiver1) = pool.activate();
    let (sender2, receiver2) = pool.activate();

    // Send values
    sender1.set(PI);
    sender2.set(E);

    // Receive values
    let result1 = poll_to_completion(receiver1);
    let result2 = poll_to_completion(receiver2);

    println!("Results: {result1:.5}, {result2:.5}");
    println!();
}

fn unsafe_pool_example() {
    println!("=== Unsafe Pool Example ===");

    // Create a pinned pool for maximum performance
    let pool = Box::pin(OnceEventPoolUnsafe::<u64>::new());

    // SAFETY: We ensure the pool outlives all sender/receiver pairs
    let (sender1, receiver1) = unsafe { pool.as_ref().activate() };
    // SAFETY: We ensure the pool outlives all sender/receiver pairs
    let (sender2, receiver2) = unsafe { pool.as_ref().activate() };

    // Send values
    sender1.set(12345);
    sender2.set(67890);

    // Receive values
    let result1 = poll_to_completion(receiver1);
    let result2 = poll_to_completion(receiver2);

    println!("Results: {result1}, {result2}");
    println!();
}
