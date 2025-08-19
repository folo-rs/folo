//! This example demonstrates how to use blind pool to turn anonymous futures
//! into named ones using trait objects.
//!
//! This is useful when you need to store futures with different concrete types
//! in a collection, but they all implement the same Future trait with the same Output.

use std::future::Future;
use std::task::{Context, Poll, Waker};

use blind_pool::{define_pooled_dyn_cast, BlindPool};

/// A simple async function that returns its input value.
async fn echo(val: u32) -> u32 {
    val
}

/// Define a custom trait for futures that return u32.
/// This demonstrates how to create a named interface for anonymous futures.
pub(crate) trait MyFuture: Future<Output = u32> {}

/// Blanket implementation - any Future<Output = u32> implements MyFuture.
impl<T> MyFuture for T where T: Future<Output = u32> {}

// Generate the casting methods for our custom trait.
define_pooled_dyn_cast!(MyFuture);

fn main() {
    println!("Demonstrating how to turn anonymous futures into named ones");
    println!("==========================================================");
    println!();

    // Create a blind pool
    let pool = BlindPool::new();

    // Insert an anonymous future and cast it to our named trait
    // This turns the anonymous future type into a known trait object type
    let mut named_future = pool.insert_mut(echo(42)).cast_my_future();

    println!("Created a named future from anonymous echo(42)");
    println!("Pool length after creating future: {}", pool.len());

    // Create a no-op waker and context for polling
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);

    println!("Starting to poll the named future...");

    // Poll the future until completion
    // The future is now a PooledMut<dyn MyFuture> instead of PooledMut<impl Future>
    let pinned_future = named_future.as_pin_mut();

    match pinned_future.poll(&mut context) {
        Poll::Ready(result) => {
            println!("Named future completed with result: {result}");
        }
        Poll::Pending => {
            println!("Named future returned Pending, which is unexpected for echo()");
        }
    }

    println!("Pool length after polling: {}", pool.len());

    // When we drop the handle, the future is automatically cleaned up
    println!("About to drop named_future...");
    drop(named_future);
    println!("Pool length after dropping named_future: {}", pool.len());

    println!();
    println!("Example completed successfully!");
}
