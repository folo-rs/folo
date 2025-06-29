//! Basic example of using events to signal between parts of an application.
//!
//! This example demonstrates the simplest usage pattern of the events package:
//! creating an event, obtaining sender and receiver endpoints, and communicating through them.

use events::once::Event;
use futures::executor::block_on;

fn main() {
    println!("=== Events Basic Example ===");

    // Create an event for passing string messages
    let event = Event::<String>::new();

    // Get both the sender and receiver endpoints
    let (sender, receiver) = event.by_ref();

    println!("Sending message through event...");
    sender.send("Hello from events!".to_string());

    println!("Receiving message through event...");
    let message = block_on(receiver.recv_async());

    println!("Received: {message}");
    println!("Example completed successfully!");
}
