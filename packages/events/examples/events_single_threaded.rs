//! Example demonstrating single-threaded event usage.
//!
//! This example shows how to use `LocalEvent<T>` for efficient single-threaded
//! event communication without thread synchronization overhead.

use std::rc::Rc;

use events::once::LocalEvent;

// Type alias for the complex data structure
type ComplexData = Vec<Rc<String>>;

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

    // Example 4: Demonstrating performance benefits (no synchronization)
    println!("\n4. Performance characteristics:");
    println!("LocalEvent uses RefCell internally - no thread synchronization overhead");
    println!("Event uses Mutex internally - thread-safe but with synchronization cost");

    let start = std::time::Instant::now();
    for i in 0..1000 {
        let event = LocalEvent::<i32>::new();
        let (sender, receiver) = event.endpoints();
        sender.send(i);
        #[allow(
            clippy::let_underscore_must_use,
            reason = "We're only timing the operations"
        )]
        let _ = receiver.receive();
    }
    let local_duration = start.elapsed();
    println!("1000 LocalEvent operations took: {local_duration:?}");

    // Example 5: Type constraints demonstration
    println!("\n5. Type constraints demonstration:");

    // LocalEvent works with any type (including !Send types)
    let _local_event_send = LocalEvent::<String>::new(); // String: Send ✓
    let _local_event_not_send = LocalEvent::<Rc<String>>::new(); // Rc<String>: !Send ✓
    println!("LocalEvent works with both Send and !Send types");

    // Example 6: Showing thread safety properties
    println!("\n6. Thread safety properties:");

    // LocalEvent does not implement Send or Sync
    // The following would not compile:
    // fn assert_send_sync<T: Send + Sync>() {}
    // assert_send_sync::<LocalEvent<String>>();

    println!("LocalEvent<T> is !Send + !Sync (single-threaded only) ✓");

    // Example 7: Nested data structures
    println!("\n7. Complex data structures:");
    let data: ComplexData = vec![
        Rc::new("First".to_string()),
        Rc::new("Second".to_string()),
        Rc::new("Third".to_string()),
    ];

    let event = LocalEvent::<ComplexData>::new();
    let (sender, receiver) = event.endpoints();
    sender.send(data.clone());
    let received_data = receiver.receive();
    println!(
        "Sent {} items, received {} items",
        data.len(),
        received_data.len()
    );
    for (i, item) in received_data.iter().enumerate() {
        println!("  Item {i}: {item}");
    }

    println!("\nSingle-threaded events example completed successfully!");
}
