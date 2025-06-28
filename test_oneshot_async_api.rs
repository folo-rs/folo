// Test file to explore oneshot async API

use std::future::Future;

fn main() {
    let (sender, receiver) = oneshot::channel::<i32>();
    
    // Check if receiver implements Future
    let _check_future: &dyn Future<Output = Result<i32, oneshot::RecvError>> = &receiver;
    
    println!("oneshot receiver implements Future!");
}
