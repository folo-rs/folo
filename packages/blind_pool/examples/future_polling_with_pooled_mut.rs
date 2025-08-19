//! This example demonstrates how to use `insert_mut()` to store and poll a Future
//! that requires mutable access through `Pin<&mut Self>`.
//!
//! Many async operations return futures that need to be polled with a mutable reference.
//! The `PooledMut` type allows us to store such futures in a pool and poll them directly.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use blind_pool::BlindPool;

/// A simple future that completes after being polled a specific number of times.
struct CountdownFuture {
    remaining_polls: u32,
}

impl CountdownFuture {
    fn new(poll_count: u32) -> Self {
        Self {
            remaining_polls: poll_count,
        }
    }
}

impl Future for CountdownFuture {
    type Output = String;

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.remaining_polls == 0 {
            Poll::Ready("Countdown completed!".to_string())
        } else {
            println!(
                "Countdown: {remaining_polls} polls remaining",
                remaining_polls = self.remaining_polls
            );
            self.remaining_polls = self.remaining_polls.saturating_sub(1);
            Poll::Pending
        }
    }
}

fn main() {
    println!("Demonstrating PooledMut with Future polling");
    println!("==========================================");
    println!();

    // Create a blind pool
    let pool = BlindPool::new();

    // Insert a future using insert_mut - this gives us exclusive mutable access
    let mut future_handle = pool.insert_mut(CountdownFuture::new(3));

    // Create a no-op waker and context for polling
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);

    println!("Starting to poll the future...");
    println!();

    // Poll the future until completion
    // We can do this because PooledMut implements DerefMut, giving us &mut access
    loop {
        // Use the built-in pinning method for pooled objects
        let pinned_future = future_handle.as_pin_mut();

        match pinned_future.poll(&mut context) {
            Poll::Ready(result) => {
                println!("Future completed with result: {result}");
                break;
            }
            Poll::Pending => {
                println!("Future returned Pending, continuing...");
                println!();
            }
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
    println!("Demonstrating insert_with_mut...");

    // SAFETY: We properly initialize the future in the closure.
    let mut future_handle2 = unsafe {
        pool.insert_with_mut(|uninit| {
            uninit.write(CountdownFuture::new(2));
        })
    };

    // Poll this one too
    let waker2 = Waker::noop();
    let mut context2 = Context::from_waker(waker2);

    loop {
        let pinned_future = future_handle2.as_pin_mut();
        match pinned_future.poll(&mut context2) {
            Poll::Ready(result) => {
                println!("Second future completed: {result}");
                break;
            }
            Poll::Pending => {
                println!("Second future pending...");
            }
        }
    }

    // Demonstrate direct polling with pinning support
    println!();
    println!("Demonstrating direct polling...");

    let mut future_handle3 = pool.insert_mut(CountdownFuture::new(1));

    // We can use the built-in pinning support for polling
    let waker3 = Waker::noop();
    let mut context3 = Context::from_waker(waker3);

    match future_handle3.as_pin_mut().poll(&mut context3) {
        Poll::Ready(result) => {
            println!("Third future completed: {result}");
        }
        Poll::Pending => {
            println!("Third future pending - polling again...");
            // Since it returned pending, let's poll once more
            match future_handle3.as_pin_mut().poll(&mut context3) {
                Poll::Ready(result) => {
                    println!("Third future completed on second poll: {result}");
                }
                Poll::Pending => {
                    println!("Third future still pending");
                }
            }
        }
    }

    println!();
    println!("All futures have been processed!");
}
