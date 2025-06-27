//! Example demonstrating different ways to obtain event endpoints.
//!
//! This example shows the various methods available for getting sender and receiver
//! endpoints from an event, including the checked variants.

use events::once::Event;

fn main() {
    println!("=== Event Endpoints Example ===");

    // Method 1: Get both endpoints at once
    println!("\n1. Using endpoints() method:");
    let event1 = Event::<i32>::new();
    let (sender1, receiver1) = event1.endpoints();

    sender1.send(42);
    let value1 = receiver1.receive();
    println!("Received: {value1}");

    // Method 2: Get endpoints separately on different events
    println!("\n2. Using separate sender() and receiver() methods:");
    let event2 = Event::<String>::new();
    let sender2 = event2.sender();

    let event3 = Event::<String>::new();
    let _receiver3 = event3.receiver();

    // Note: These are from different events, so sender2 and receiver3 are not connected
    sender2.send("Message to nowhere".to_string());
    println!("Sent message from event2 (no receiver waiting)");

    // receiver3 would wait forever if we called receive() since no sender for event3
    println!("Not calling receive() on event3 receiver to avoid infinite wait");

    // Method 3: Using checked variants
    println!("\n3. Using checked endpoint methods:");
    let event4 = Event::<u64>::new();

    if let Some((sender4, receiver4)) = event4.endpoints_checked() {
        sender4.send(12345);
        let value4 = receiver4.receive();
        println!("Received via checked method: {value4}");
    }

    // Try to get endpoints again - should return None
    if event4.endpoints_checked().is_some() {
        println!("This should not print - endpoints already retrieved!");
    } else {
        println!("Correctly returned None when trying to get endpoints again");
    }

    println!("\nExample completed successfully!");
}
