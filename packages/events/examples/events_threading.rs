//! Example demonstrating both thread-safe and single-threaded event variants.
//!
//! This example shows the difference between Event<T> (thread-safe) and LocalEvent<T>
//! (single-threaded), and how to use them appropriately.

use events::once::{Event, LocalEvent};
use std::rc::Rc;
use std::sync::Arc;

fn main() {
    println!("=== Thread-safe vs Single-threaded Events Example ===");

    // Example 1: Thread-safe Event used within the same thread
    println!("\n1. Thread-safe Event used in same thread:");
    let event = Event::<String>::new();
    let (sender, receiver) = event.endpoints();
    sender.send("Hello from thread-safe event!".to_string());
    let message = receiver.receive();
    println!("Received: {}", message);

    // Example 2: Thread-safe Event can be wrapped in Arc
    println!("\n2. Thread-safe Event wrapped in Arc:");
    let event_arc = Arc::new(Event::<i32>::new());
    // You can clone the Arc but each Event can only have endpoints retrieved once
    let _event_clone = Arc::clone(&event_arc);
    let (sender, receiver) = event_arc.endpoints();
    sender.send(42);
    let value = receiver.receive();
    println!("Received from Arc-wrapped event: {}", value);

    // Example 3: Thread-safe Event can also be used with Rc
    println!("\n3. Thread-safe Event with Rc for single-threaded usage:");
    let event_rc = Rc::new(Event::<Vec<u8>>::new());
    let (sender_rc, receiver_rc) = event_rc.endpoints();
    sender_rc.send(vec![1, 2, 3, 4, 5]);
    let vec_data = receiver_rc.receive();
    println!("Received from Rc-wrapped event: {:?}", vec_data);

    // Example 4: Single-threaded LocalEvent with Rc (cannot be used with Arc)
    println!("\n4. Single-threaded LocalEvent with Rc:");
    let local_event = Rc::new(LocalEvent::<String>::new());
    let (local_sender, local_receiver) = local_event.endpoints();
    local_sender.send("Hello from local event!".to_string());
    let local_message = local_receiver.receive();
    println!("Received from LocalEvent: {}", local_message);

    // Example 5: Demonstrating type constraints
    println!("\n5. Type constraints demonstration:");
    
    // Thread-safe Event requires T: Send
    let _thread_safe_event = Event::<String>::new(); // String: Send ✓
    
    // Single-threaded LocalEvent works with any type (including !Send types)
    let _local_event = LocalEvent::<Rc<String>>::new(); // Rc<String>: !Send ✓
    
    println!("Both events created successfully with their respective type constraints");

    // Example 6: Showing thread safety properties
    println!("\n6. Thread safety properties:");
    
    // Event<T> implements Send + Sync when T: Send
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Event<String>>();
    println!("Event<String> is Send + Sync ✓");
    
    // LocalEvent<T> does not implement Send or Sync
    // The following would not compile:
    // assert_send_sync::<LocalEvent<String>>();
    println!("LocalEvent<T> is !Send + !Sync (single-threaded only) ✓");

    println!("\nExample completed successfully!");
}