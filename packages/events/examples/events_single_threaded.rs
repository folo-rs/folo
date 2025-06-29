//! Example demonstrating single-threaded event usage.
//!
//! This example shows how to use `LocalEvent<T>` for efficient single-threaded
//! event communication without thread synchronization overhead.

use std::rc::Rc;

use events::once::LocalEvent;

fn main() {
    println!("=== Single-threaded Events Example ===");

    // Example 1: Basic LocalEvent usage
    println!("\n1. Basic LocalEvent usage:");
    let event = LocalEvent::<String>::new();
    let (sender, receiver) = event.endpoints();
    sender.send("Hello from local event!".to_string());
    let message = receiver.receive();
    println!("Received: {message}");

    // Example 2: LocalEvent with Rc for sharing (single-threaded only)
    println!("\n2. LocalEvent with Rc for sharing:");
    let event_rc = Rc::new(LocalEvent::<i32>::new());
    let (sender_rc, receiver_rc) = event_rc.endpoints();
    sender_rc.send(123);
    let value = receiver_rc.receive();
    println!("Received from Rc-wrapped LocalEvent: {value}");

    // Example 3: LocalEvent works with !Send types
    println!("\n3. LocalEvent works with !Send types:");
    let rc_data = Rc::new("Shared data in Rc".to_string());
    let event = LocalEvent::<Rc<String>>::new();
    let (sender, receiver) = event.endpoints();
    sender.send(Rc::<String>::clone(&rc_data));
    let received_rc = receiver.receive();
    println!("Received Rc data: {received_rc}");
    println!("Original Rc data: {rc_data}");

    // Example 4: Type constraints demonstration
    println!("\n4. Type constraints demonstration:");

    // LocalEvent works with any type (including !Send types)
    let _local_event_send = LocalEvent::<String>::new(); // String: Send ✓
    let _local_event_not_send = LocalEvent::<Rc<String>>::new(); // Rc<String>: !Send ✓
    println!("LocalEvent works with both Send and !Send types");

    println!("\nSingle-threaded events example completed successfully!");
}
