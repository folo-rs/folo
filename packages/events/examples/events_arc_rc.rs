//! Example demonstrating Arc and Rc-based event usage.

use std::f64::consts::PI;
use std::rc::Rc;
use std::sync::Arc;
use std::thread;

use events::once::{Event, LocalEvent};

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
    let event = Arc::new(Event::<String>::new());
    let (sender, receiver) = event.by_arc();

    sender.send("Hello from Arc Event!".to_string());
    let message = receiver.recv();
    println!("Received: {message}");
}

/// Demonstrates Rc-based event usage.
fn rc_event_example() {
    let event = Rc::new(Event::<i32>::new());
    let (sender, receiver) = event.by_rc();

    sender.send(42);
    let value = receiver.recv();
    println!("Received: {value}");
}

/// Demonstrates Rc-based `LocalEvent` usage.
fn rc_local_event_example() {
    let event = Rc::new(LocalEvent::<f64>::new());
    let (sender, receiver) = event.by_rc();

    sender.send(PI);
    let value = receiver.recv();
    println!("Received: {value:.5}");
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
        let message = receiver.recv();
        println!("Received in thread: {message}");
        message
    });

    // Wait for completion
    sender_handle.join().unwrap();
    let message = receiver_handle.join().unwrap();
    assert_eq!(message, "Message from another thread!");
}
