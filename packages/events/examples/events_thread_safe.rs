//! Example demonstrating thread-safe event usage.
//!
//! This example shows how to use Event<T> (thread-safe) in various scenarios,
//! including cross-thread communication and different wrapper types.

use std::sync::Arc;
use std::thread;

use events::once::Event;

fn main() {
    println!("=== Thread-safe Events Example ===");

    // Example 1: Thread-safe Event used within the same thread
    println!("\n1. Thread-safe Event used in same thread:");
    let event = Event::<String>::new();
    let (sender, receiver) = event.endpoints();
    sender.send("Hello from thread-safe event!".to_string());
    let message = receiver.receive();
    println!("Received: {message}");

    // Example 2: Thread-safe Event can be wrapped in Arc for sharing
    println!("\n2. Thread-safe Event wrapped in Arc:");
    let event_arc = Arc::new(Event::<i32>::new());
    // You can clone the Arc but each Event can only have endpoints retrieved once
    let _event_clone = Arc::clone(&event_arc);
    let (sender, receiver) = event_arc.endpoints();
    sender.send(42);
    let value = receiver.receive();
    println!("Received from Arc-wrapped event: {value}");

    // Example 3: Cross-thread communication with Event endpoints
    println!("\n3. Cross-thread communication:");
    let event = Event::<String>::new();
    let (sender, receiver) = event.endpoints();

    let sender_handle = thread::spawn(move || {
        println!("Sender thread: Sending message...");
        sender.send("Hello from another thread!".to_string());
        println!("Sender thread: Message sent");
    });

    let receiver_handle = thread::spawn(move || {
        println!("Receiver thread: Waiting for message...");
        let message = receiver.receive();
        println!("Receiver thread: Received: {message}");
        message
    });

    sender_handle.join().unwrap();
    let cross_thread_message = receiver_handle.join().unwrap();
    println!("Main thread received result: {cross_thread_message}");

    println!("\nThread-safe events example completed successfully!");
}
