//! Test example for oneshot async functionality.

use futures::channel::oneshot;
use futures::executor::block_on;

fn main() {
    let (sender, receiver) = oneshot::channel::<i32>();

    // Test if receiver implements Future
    let async_task = async move {
        let result = receiver.await;
        println!("Received: {result:?}");
    };

    // Send a value
    sender.send(42).unwrap();

    // Run the async task
    block_on(async_task);
}
