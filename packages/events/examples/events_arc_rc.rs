//! Example demonstrating Arc and Rc-based event usage.

use std::f64::consts::PI;
use std::rc::Rc;
use std::sync::Arc;
use std::thread;

use events::{LocalOnceEvent, OnceEvent};
use futures::executor::block_on;

fn main() {
    println!("=== Arc-based Event Example ===");
    arc_event_example();

    println!("\n=== Rc-based LocalEvent Example ===");
    rc_local_event_example();

    println!("\n=== Cross-thread Arc Event Example ===");
    cross_thread_arc_example();
}

/// Demonstrates Arc-based event usage in single thread.
fn arc_event_example() {
    block_on(async {
        let event = Arc::new(OnceEvent::<String>::new());
        let (sender, receiver) = event.bind_by_arc();

        sender.send("Hello from Arc Event!".to_string());
        let message = receiver.await.expect(
            "sender.send() was called immediately before this, so sender cannot be dropped",
        );
        println!("Received: {message}");
    });
}

/// Demonstrates Rc-based `LocalEvent` usage.
fn rc_local_event_example() {
    block_on(async {
        let event = Rc::new(LocalOnceEvent::<f64>::new());
        let (sender, receiver) = event.bind_by_rc();

        sender.send(PI);
        let value = receiver.await.expect(
            "sender.send() was called immediately before this, so sender cannot be dropped",
        );
        println!("Received: {value:.5}");
    });
}

/// Demonstrates cross-thread communication with Arc events.
fn cross_thread_arc_example() {
    let event = Arc::new(OnceEvent::<String>::new());
    let (sender, receiver) = event.bind_by_arc();

    // Send from one thread
    let sender_handle = thread::spawn(move || {
        sender.send("Message from another thread!".to_string());
        println!("Message sent from thread");
    });

    // Receive from another thread
    let receiver_handle = thread::spawn(move || {
        let message =
            block_on(receiver).expect("sender thread is guaranteed to call send() before joining");
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
