//! Example demonstrating thread-safe event usage.
//!
//! This example shows how to use Event<T> (thread-safe) in various scenarios,
//! including cross-thread communication and different wrapper types.

use std::sync::Arc;
use std::thread;

use events::OnceEvent;
use futures::executor::block_on;

fn main() {
    println!("=== Thread-safe Events Example ===");

    block_on(async {
        // Example 1: Thread-safe Event used within the same thread
        println!();
        println!("1. Thread-safe Event used in same thread:");
        let event = OnceEvent::<String>::new();
        let (sender, receiver) = event.bind_by_ref();
        sender.send("Hello from thread-safe event!".to_string());
        let message = receiver.await.expect(
            "sender.send() was called immediately before this, so sender cannot be dropped",
        );
        println!("Received: {message}");

        // Example 2: Thread-safe Event can be wrapped in Arc for sharing
        println!();
        println!("2. Thread-safe Event wrapped in Arc:");
        let event_arc = Arc::new(OnceEvent::<i32>::new());
        // You can clone the Arc but each Event can only have endpoints retrieved once
        let _event_clone = Arc::clone(&event_arc);
        let (sender, receiver) = event_arc.bind_by_ref();
        sender.send(42);
        let value = receiver.await.expect(
            "sender.send() was called immediately before this, so sender cannot be dropped",
        );
        println!("Received from Arc-wrapped event: {value}");
    });

    // Example 3: Cross-thread pattern - endpoints extracted before threading
    // Note: With Ref types, the Event must live until threads complete.
    // This pattern avoids the lifetime issue by using a scoped approach.
    println!();
    println!("3. Cross-thread communication pattern:");

    // Extract endpoints before threading to satisfy lifetime requirements
    let event = OnceEvent::<String>::new();
    let (sender, receiver) = event.bind_by_ref();

    // Use scoped threads to ensure Event lives long enough
    thread::scope(|s| {
        let sender_handle = s.spawn(|| {
            println!("Sender thread: Sending message...");
            sender.send("Hello from scoped thread!".to_string());
            println!("Sender thread: Message sent");
        });

        let receiver_handle = s.spawn(|| {
            println!("Receiver thread: Waiting for message...");
            let message = block_on(receiver)
                .expect("scoped threads guarantee sender thread completes before scope ends");
            println!("Receiver thread: Received: {message}");
            message
        });

        sender_handle
            .join()
            .expect("sender thread should complete successfully");
        let message = receiver_handle
            .join()
            .expect("receiver thread should complete successfully");
        println!("Cross-thread message: {message}");
    });

    // Event is safely dropped here after all threads complete

    println!();
    println!("Thread-safe events example completed successfully!");
}
