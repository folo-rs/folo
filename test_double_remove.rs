use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

use blind_pool::{BlindPool, PooledMut};

async fn echo(val: u32) -> u32 {
    val
}

fn main() {
    println!("Starting to poll the future...");
    
    let pool = BlindPool::new();
    let mut future_handle = pool.insert_mut(echo(10));
    
    // Create a context for polling
    let waker = Waker::noop();
    let mut context = Context::from_waker(&waker);
    
    // Poll the future
    let pinned_future = future_handle.as_pin_mut();
    match pinned_future.poll(&mut context) {
        Poll::Ready(result) => {
            println!("Future completed with result: {result}");
        }
        Poll::Pending => {
            println!("Future is still pending...");
        }
    }
    
    println!();
    println!("Example completed!");
}