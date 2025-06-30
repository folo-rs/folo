//! Basic example of using events to signal between parts of an application.
//!
//! This example demonstrates the simplest usage pattern of the events package:
//! creating an event, obtaining sender and receiver endpoints, and communicating through them.

use events::OnceEvent;
use futures::executor::block_on;

fn main() {
    println!("=== Events Basic Example ===");

    block_on(async {
        // Event for passing string messages between application components
        let event = OnceEvent::<String>::new();

        // Extract both communication endpoints
        let (sender, receiver) = event.bind_by_ref();

        println!("Sending message through event...");
        sender.send("Hello from events!".to_string());

        println!("Receiving message through event...");
        let message = receiver.await.expect(
            "sender.send() was called immediately before this, so sender cannot be dropped",
        );

        println!("Received: {message}");
        println!("Example completed successfully!");
    });
}
