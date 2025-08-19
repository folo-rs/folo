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
            println!("Countdown: {remaining_polls} polls remaining", remaining_polls = self.remaining_polls);
            self.remaining_polls = self.remaining_polls.saturating_sub(1);
            Poll::Pending
        }
    }
}

/// A dummy waker for this example.
struct DummyWaker;

impl std::task::Wake for DummyWaker {
    fn wake(self: std::sync::Arc<Self>) {
        // In a real async runtime, this would wake up the executor
    }
}

fn main() {
    println!("Demonstrating PooledMut with Future polling");
    println!("==========================================\n");

    // Create a blind pool
    let pool = BlindPool::new();

    // Insert a future using insert_mut - this gives us exclusive mutable access
    let mut future_handle = pool.insert_mut(CountdownFuture::new(3));

    // Create a dummy waker and context for polling
    let waker = Waker::from(std::sync::Arc::new(DummyWaker));
    let mut context = Context::from_waker(&waker);

    println!("Starting to poll the future...\n");

    // Poll the future until completion
    // We can do this because PooledMut implements DerefMut, giving us &mut access
    loop {
        // Pin the mutable reference to poll the future
        let pinned_future = Pin::new(&mut *future_handle);

        match pinned_future.poll(&mut context) {
            Poll::Ready(result) => {
                println!("Future completed with result: {result}");
                break;
            }
            Poll::Pending => {
                println!("Future returned Pending, continuing...\n");
            }
        }
    }

    println!("\nExample completed!");
    println!("Pool length before cleanup: {}", pool.len());

    // When we drop the handle, the future is automatically cleaned up
    drop(future_handle);
    println!("Pool length after cleanup: {}", pool.len());

    // Demonstrate that we can also use insert_with_mut for in-place construction
    println!("\nDemonstrating insert_with_mut...");

    // SAFETY: We properly initialize the future in the closure.
    let mut future_handle2 = unsafe {
        pool.insert_with_mut(|uninit| {
            uninit.write(CountdownFuture::new(2));
        })
    };

    // Poll this one too
    let waker2 = Waker::from(std::sync::Arc::new(DummyWaker));
    let mut context2 = Context::from_waker(&waker2);

    loop {
        let pinned_future = Pin::new(&mut *future_handle2);
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

    // Demonstrate type casting to Future trait object
    println!("\nDemonstrating casting to trait object...");

    let mut future_handle3 = pool.insert_mut(CountdownFuture::new(1));

    // We can cast to a trait object for dynamic dispatch
    // Note: We use Pin::new_unchecked here because we know the future won't move
    let future_ref: &mut dyn Future<Output = String> = &mut *future_handle3;
    // SAFETY: We know the future is pinned and won't move because it's stored in the pool.
    let pinned_trait_future = unsafe { Pin::new_unchecked(future_ref) };

    let waker3 = Waker::from(std::sync::Arc::new(DummyWaker));
    let mut context3 = Context::from_waker(&waker3);

    match pinned_trait_future.poll(&mut context3) {
        Poll::Ready(result) => {
            println!("Trait object future completed: {result}");
        }
        Poll::Pending => {
            println!("Trait object future pending - polling again...");
            // Since it returned pending, let's poll once more
            let future_ref2: &mut dyn Future<Output = String> = &mut *future_handle3;
            // SAFETY: We know the future is pinned and won't move because it's stored in the pool.
            let pinned_trait_future2 = unsafe { Pin::new_unchecked(future_ref2) };
            match pinned_trait_future2.poll(&mut context3) {
                Poll::Ready(result) => {
                    println!("Trait object future completed on second poll: {result}");
                }
                Poll::Pending => {
                    println!("Trait object future still pending");
                }
            }
        }
    }

    println!("\nAll futures have been processed!");
}
