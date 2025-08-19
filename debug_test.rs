use std::future::Future;
use std::task::{Context, Poll, Waker};

use blind_pool::{define_pooled_dyn_cast, BlindPool};

// Test with a simple non-Send future to see if that avoids the problem
pub(crate) trait MyFuture: Future<Output = u32> {}
impl<T> MyFuture for T where T: Future<Output = u32> {}

define_pooled_dyn_cast!(MyFuture);

async fn echo(val: u32) -> u32 {
    val
}

fn main() {
    println!("Simple test");
    
    let pool = BlindPool::new();
    let mut future_handle = pool.insert_mut(echo(10));
    
    // Check if we can cast
    let mut casted = future_handle.cast_my_future();
    
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    
    match casted.as_pin_mut().poll(&mut context) {
        Poll::Ready(result) => {
            println!("Result: {}", result);
        }
        Poll::Pending => {
            println!("Pending");
        }
    }
    
    println!("About to drop");
    drop(casted);
    println!("Dropped successfully");
}