//! Test without casting to verify the issue is in the casting logic

use std::future::Future;
use std::task::{Context, Poll, Waker};

use blind_pool::BlindPool;

async fn echo(val: u32) -> u32 {
    val
}

fn main() {
    println!("Testing without casting");
    
    let pool = BlindPool::new();
    
    // Don't cast, just use the future directly
    let mut future_handle = pool.insert_mut(echo(42));
    
    println!("Pool length after creating future: {}", pool.len());
    
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    
    let pinned_future = future_handle.as_pin_mut();
    match pinned_future.poll(&mut context) {
        Poll::Ready(result) => {
            println!("Future completed with result: {result}");
        }
        Poll::Pending => {
            println!("Future pending");
        }
    }
    
    println!("Pool length after polling: {}", pool.len());
    
    println!("About to drop...");
    drop(future_handle);
    println!("Pool length after drop: {}", pool.len());
    
    println!("Success!");
}