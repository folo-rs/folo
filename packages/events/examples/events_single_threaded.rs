//! Example demonstrating single-threaded event usage.
//!
//! This example shows how to use `LocalEvent<T>` for efficient single-threaded
//! event communication without thread synchronization overhead.

use std::rc::Rc;

use events::LocalOnceEvent;
use futures::executor::block_on;

/// Example integer value for testing.
const EXAMPLE_INTEGER: i32 = 123;

fn main() {
    println!("=== Single-threaded Events Example ===");

    block_on(async {
        // Example 1: Basic LocalEvent usage
        println!();
        println!("1. Basic LocalEvent usage:");
        let event = LocalOnceEvent::<String>::new();
        let (sender, receiver) = event.bind_by_ref();
        sender.send("Hello from local event!".to_string());
        let message = receiver.await.expect("sender.send() was called immediately before this, so sender cannot be dropped");
        println!("Received: {message}");

        // Example 2: LocalEvent with Rc for sharing (single-threaded only)
        println!();
        println!("2. LocalEvent with Rc for sharing:");
        let event_rc = Rc::new(LocalOnceEvent::<i32>::new());
        let (sender_rc, receiver_rc) = event_rc.bind_by_ref();
        sender_rc.send(EXAMPLE_INTEGER);
        let value = receiver_rc.await.expect("sender_rc.send() was called immediately before this, so sender cannot be dropped");
        println!("Received from Rc-wrapped LocalEvent: {value}");

        // Example 3: LocalEvent works with !Send types
        println!();
        println!("3. LocalEvent works with !Send types:");
        let rc_data = Rc::new("Shared data in Rc".to_string());
        let event = LocalOnceEvent::<Rc<String>>::new();
        let (sender, receiver) = event.bind_by_ref();
        sender.send(Rc::<String>::clone(&rc_data));
        let received_rc = receiver.await.expect("sender.send() was called immediately before this, so sender cannot be dropped");
        println!("Received Rc data: {received_rc}");
        println!("Original Rc data: {rc_data}");

        // Example 4: Type constraints demonstration
        println!();
        println!("4. Type constraints demonstration:");

        // LocalEvent works with any type (including !Send types)
        let _local_event_send = LocalOnceEvent::<String>::new(); // String: Send ✓
        let _local_event_not_send = LocalOnceEvent::<Rc<String>>::new(); // Rc<String>: !Send ✓
        println!("LocalEvent works with both Send and !Send types");

        println!();
        println!("Single-threaded events example completed successfully!");
    });
}
