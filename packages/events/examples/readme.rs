//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This shows how to use `OnceEvent` for thread-safe event signaling as documented.

use events::OnceEvent;
use futures::executor::block_on;

fn main() {
    println!("=== Events README Example ===");
    
    block_on(async {
        // Create a thread-safe event for passing a string message
        let event = OnceEvent::<String>::new();
        let (sender, receiver) = event.bind_by_ref();

        // Send a message through the event
        sender.send("Hello, World!".to_string());

        // Receive the message (await since it's async)
        let message = receiver.await.unwrap();
        assert_eq!(message, "Hello, World!");
        
        println!("Received message: {message}");
        println!("README example completed successfully!");
    });
}
