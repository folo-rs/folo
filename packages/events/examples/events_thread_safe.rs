//! Example demonstrating thread-safe event usage.
//!
//! This example shows how to use Event<T> (thread-safe) in various scenarios,
//! including cross-thread communication and different wrapper types.

use events::once::Event;
use std::rc::Rc;
use std::sync::Arc;
use std::thread;

fn main() {
    println!("=== Thread-safe Events Example ===");

    // Example 1: Thread-safe Event used within the same thread
    println!("\n1. Thread-safe Event used in same thread:");
    let event = Event::<String>::new();
    let (sender, receiver) = event.endpoints();
    sender.send("Hello from thread-safe event!".to_string());
    let message = receiver.receive();
    println!("Received: {}", message);

    // Example 2: Thread-safe Event can be wrapped in Arc for sharing
    println!("\n2. Thread-safe Event wrapped in Arc:");
    let event_arc = Arc::new(Event::<i32>::new());
    // You can clone the Arc but each Event can only have endpoints retrieved once
    let _event_clone = Arc::clone(&event_arc);
    let (sender, receiver) = event_arc.endpoints();
    sender.send(42);
    let value = receiver.receive();
    println!("Received from Arc-wrapped event: {}", value);

    // Example 3: Thread-safe Event can also be used with Rc for single-threaded usage
    println!("\n3. Thread-safe Event with Rc for single-threaded usage:");
    let event_rc = Rc::new(Event::<Vec<u8>>::new());
    let (sender_rc, receiver_rc) = event_rc.endpoints();
    sender_rc.send(vec![1, 2, 3, 4, 5]);
    let vec_data = receiver_rc.receive();
    println!("Received from Rc-wrapped event: {:?}", vec_data);

    // Example 4: Cross-thread communication with Arc
    println!("\n4. Cross-thread communication:");
    let event = Arc::new(Event::<String>::new());
    let (sender, receiver) = event.endpoints();

    // In a real scenario, you might pass the event around differently
    // Here we demonstrate that the sender and receiver can be used across threads
    
    let sender_handle = thread::spawn(move || {
        println!("Sender thread: Sending message...");
        sender.send("Hello from another thread!".to_string());
        println!("Sender thread: Message sent");
    });

    let receiver_handle = thread::spawn(move || {
        println!("Receiver thread: Waiting for message...");
        let message = receiver.receive();
        println!("Receiver thread: Received: {}", message);
        message
    });

    sender_handle.join().unwrap();
    let cross_thread_message = receiver_handle.join().unwrap();
    println!("Main thread received result: {}", cross_thread_message);

    // Example 5: Demonstrating type constraints
    println!("\n5. Type constraints demonstration:");
    
    // Thread-safe Event requires T: Send
    let _thread_safe_event = Event::<String>::new(); // String: Send ✓
    println!("Event<String> created successfully (String is Send)");
    
    // The following would not compile because Rc<String> is !Send:
    // let _invalid_event = Event::<Rc<String>>::new(); // Rc<String>: !Send ✗

    // Example 6: Showing thread safety properties
    println!("\n6. Thread safety properties:");
    
    // Event<T> implements Send + Sync when T: Send
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Event<String>>();
    println!("Event<String> is Send + Sync ✓");

    println!("\nThread-safe events example completed successfully!");
}