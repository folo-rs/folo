//! Example demonstrating Arc and Rc-based event usage.

use std::f64::consts::PI;
use std::rc::Rc;
use std::sync::Arc;
use std::thread;

use events::once::{Event, LocalEvent};
use futures::executor::block_on;

/// Example integer value for testing.
const EXAMPLE_INTEGER: i32 = 42;

fn main() {
    println!("=== Arc-based Event Example ===");
    arc_event_example();

    println!("\n=== Rc-based Event Example ===");
    rc_event_example();

    println!("\n=== Rc-based LocalEvent Example ===");
    rc_local_event_example();

    println!("\n=== Cross-thread Arc Event Example ===");
    cross_thread_arc_example();
}

/// Demonstrates Arc-based event usage in single thread.
fn arc_event_example() {
    block_on(async {
        let event = Arc::new(Event::<String>::new());
        let (sender, receiver) = event.by_arc();

        sender.send("Hello from Arc Event!".to_string());
        let message = receiver.await;
        println!("Received: {message}");
    });
}

/// Demonstrates Rc-based event usage.
fn rc_event_example() {
    block_on(async {
        let event = Rc::new(Event::<i32>::new());
        let (sender, receiver) = event.by_rc();

        sender.send(EXAMPLE_INTEGER);
        let value = receiver.await;
        println!("Received: {value}");
    });
}

/// Demonstrates Rc-based `LocalEvent` usage.
fn rc_local_event_example() {
    block_on(async {
        let event = Rc::new(LocalEvent::<f64>::new());
        let (sender, receiver) = event.by_rc();

        sender.send(PI);
        let value = receiver.await;
        println!("Received: {value:.5}");
    });
}

/// Demonstrates cross-thread communication with Arc events.
fn cross_thread_arc_example() {
    let event = Arc::new(Event::<String>::new());
    let (sender, receiver) = event.by_arc();

    // Send from one thread
    let sender_handle = thread::spawn(move || {
        sender.send("Message from another thread!".to_string());
        println!("Message sent from thread");
    });

    // Receive from another thread
    let receiver_handle = thread::spawn(move || {
        let message = block_on(receiver);
        println!("Received in thread: {message}");
        message
    });

    // Wait for completion
    sender_handle
        .join()
        .expect("sender thread should complete successfully");
    let message = receiver_handle
        .join()
        .expect("receiver thread should complete successfully");
    assert_eq!(message, "Message from another thread!");
}
