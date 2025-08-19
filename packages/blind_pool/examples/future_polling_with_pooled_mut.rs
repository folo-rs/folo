//! This example demonstrates how to use `insert_mut()` to store and poll futures
//! that require mutable access through `Pin<&mut Self>`.
//!
//! Many async operations return futures that need to be polled with a mutable reference.
//! The `PooledMut` type allows us to store such futures in a pool and poll them directly.

use std::future::Future;
use std::task::{Context, Poll, Waker};

use blind_pool::BlindPool;

/// A simple async function that returns its input value.
/// This simulates a future that might be returned from an actual async operation.
#[allow(
    clippy::unused_async,
    reason = "Intentionally async to demonstrate future handling even though no await is used"
)]
async fn echo(val: u32) -> u32 {
    // Simulate some async work (even if it completes immediately)
    val
}

fn main() {
    println!("Demonstrating PooledMut with Future polling");
    println!("==========================================");
    println!();

    // Create a blind pool
    let pool = BlindPool::new();

    // Insert an anonymous future using insert_mut - this gives us exclusive mutable access
    let mut future_handle = pool.insert_mut(echo(42));

    // Create a no-op waker and context for polling
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);

    println!("Starting to poll the echo future...");
    println!();

    // Poll the future until completion
    // We can do this because PooledMut implements DerefMut, giving us &mut access
    let pinned_future = future_handle.as_pin_mut();

    match pinned_future.poll(&mut context) {
        Poll::Ready(result) => {
            println!("Future completed with result: {result}");
        }
        Poll::Pending => {
            println!("Future returned Pending, which is unexpected for echo()");
        }
    }

    println!();
    println!("Example completed!");
    println!("Pool length before cleanup: {}", pool.len());

    // When we drop the handle, the future is automatically cleaned up
    drop(future_handle);
    println!("Pool length after cleanup: {}", pool.len());

    // Demonstrate that we can also use insert_with_mut for in-place construction
    println!();
    println!("Demonstrating insert_with_mut with another echo future...");

    // SAFETY: We properly initialize the future in the closure by calling write with a valid value.
    let mut future_handle2 = unsafe {
        pool.insert_with_mut(|uninit| {
            uninit.write(echo(123));
        })
    };

    // Poll this one too
    let waker2 = Waker::noop();
    let mut context2 = Context::from_waker(waker2);

    let pinned_future = future_handle2.as_pin_mut();
    match pinned_future.poll(&mut context2) {
        Poll::Ready(result) => {
            println!("Second future completed: {result}");
        }
        Poll::Pending => {
            println!("Second future pending...");
        }
    }

    // Demonstrate direct polling with pinning support
    println!();
    println!("Demonstrating direct polling with a different value...");

    let mut future_handle3 = pool.insert_mut(echo(999));

    // We can use the built-in pinning support for polling
    let waker3 = Waker::noop();
    let mut context3 = Context::from_waker(waker3);

    match future_handle3.as_pin_mut().poll(&mut context3) {
        Poll::Ready(result) => {
            println!("Third future completed: {result}");
        }
        Poll::Pending => {
            println!("Third future pending - this is unexpected for echo()");
        }
    }

    println!();
    println!("All futures have been processed!");
}
