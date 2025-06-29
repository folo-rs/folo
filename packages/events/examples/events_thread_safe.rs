//! Example demonstrating thread-safe event usage.
//!
//! This example shows how to use Event<T> (thread-safe) in various scenarios,
//! including cross-thread communication and different wrapper types.

use std::sync::Arc;
use std::thread;

use events::once::Event;
use futures::executor::block_on;

fn main() {
    println!("=== Thread-safe Events Example ===");

    // Example 1: Thread-safe Event used within the same thread
    println!();
    println!("1. Thread-safe Event used in same thread:");
    let event = Event::<String>::new();
    let (sender, receiver) = event.by_ref();
    sender.send("Hello from thread-safe event!".to_string());
    let message = block_on(receiver);
    println!("Received: {message}");

    // Example 2: Thread-safe Event can be wrapped in Arc for sharing
    println!();
    println!("2. Thread-safe Event wrapped in Arc:");
    let event_arc = Arc::new(Event::<i32>::new());
    // You can clone the Arc but each Event can only have endpoints retrieved once
    let _event_clone = Arc::clone(&event_arc);
    let (sender, receiver) = event_arc.by_ref();
    sender.send(42);
    let value = block_on(receiver);
    println!("Received from Arc-wrapped event: {value}");

    // Example 3: Cross-thread pattern - endpoints extracted before threading
    // Note: With ByRef types, the Event must live until threads complete.
    // This pattern avoids the lifetime issue by using a scoped approach.
    println!();
    println!("3. Cross-thread communication pattern:");

    // Create event and extract endpoints in the main scope
    let event = Event::<String>::new();
    let (sender, receiver) = event.by_ref();

    // Use scoped threads to ensure Event lives long enough
    thread::scope(|s| {
        let sender_handle = s.spawn(|| {
            println!("Sender thread: Sending message...");
            sender.send("Hello from scoped thread!".to_string());
            println!("Sender thread: Message sent");
        });

        let receiver_handle = s.spawn(|| {
            println!("Receiver thread: Waiting for message...");
            let message = block_on(receiver);
            println!("Receiver thread: Received: {message}");
            message
        });

        sender_handle.join().unwrap();
        let message = receiver_handle.join().unwrap();
        println!("Cross-thread message: {message}");
    });

    // Event is safely dropped here after all threads complete

    println!();
    println!("Thread-safe events example completed successfully!");
}
