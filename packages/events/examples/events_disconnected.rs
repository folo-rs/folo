//! Example demonstrating the disconnected state functionality in events.

use events::{LocalOnceEvent, OnceEvent};
use futures::executor::block_on;

fn main() {
    println!("=== Events Disconnected State Example ===");

    // Example 1: Normal operation (sender not dropped)
    println!("\n1. Normal operation - sender sends value:");
    {
        block_on(async {
            let event = OnceEvent::<i32>::new();
            let (sender, receiver) = event.bind_by_ref();

            sender.send(42);
            let result = receiver.await;
            match result {
                Ok(value) => println!("   Received value: {value}"),
                Err(_) => println!("   Sender was disconnected"),
            }
        });
    }

    // Example 2: Disconnected state (sender dropped before sending)
    println!("\n2. Disconnected state - sender dropped before sending:");
    {
        block_on(async {
            let event = OnceEvent::<i32>::new();
            let (sender, receiver) = event.bind_by_ref();

            // Drop the sender without sending any value
            drop(sender);

            let result = receiver.await;
            match result {
                Ok(value) => println!("   Received value: {value}"),
                Err(_) => println!("   ✓ Sender was disconnected (as expected)"),
            }
        });
    }

    // Example 3: Local event with disconnected state
    println!("\n3. Local event with disconnected state:");
    {
        block_on(async {
            let event = LocalOnceEvent::<String>::new();
            let (sender, receiver) = event.bind_by_ref();

            // Drop the sender without sending any value
            drop(sender);

            let result = receiver.await;
            match result {
                Ok(value) => println!("   Received value: {value}"),
                Err(_) => println!("   ✓ Sender was disconnected (as expected)"),
            }
        });
    }

    // Example 4: Real-world scenario - timeout vs disconnection
    println!("\n4. Real-world scenario - handling both success and disconnection:");
    {
        block_on(async {
            let event = OnceEvent::<String>::new();
            let (sender, receiver) = event.bind_by_ref();

            // Simulate a scenario where we might not know if sender will send or be dropped
            let will_send = false; // Change to true to see success case

            if will_send {
                sender.send("Success message!".to_string());
            } else {
                drop(sender); // Simulate sender being dropped
            }

            let result = receiver.await;
            match result {
                Ok(message) => println!("   ✓ Operation succeeded: {message}"),
                Err(_) => println!("   ✓ Operation cancelled (sender disconnected)"),
            }
        });
    }

    println!("\n=== Key Benefits of Disconnected State ===");
    println!("✓ Prevents receivers from hanging indefinitely when senders are dropped");
    println!("✓ Provides clear error indication via Result<T, Disconnected>");
    println!("✓ Allows graceful handling of cancelled operations");
    println!("✓ Works consistently across all event variants (sync, local, pooled)");
    println!("\n=== Usage Guidelines ===");
    println!("• Use .unwrap() in examples/tests where disconnection is unexpected");
    println!("• Use .expect(\"custom message\") for better error context");
    println!("• Use match/if-let for scenarios where disconnection is a valid outcome");
    println!("• Handle both Ok(value) and Err(Disconnected) cases appropriately");
}
