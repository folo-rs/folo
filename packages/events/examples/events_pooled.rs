//! Example demonstrating pooled events with automatic resource management.

use events::OnceEventPool;
use futures::executor::block_on;

/// Example integer value for testing.
const EXAMPLE_INTEGER: i32 = 42;

/// Example second value for testing.
const EXAMPLE_SECOND_VALUE: i32 = 100;

fn main() {
    println!("=== Pooled Events Example ===");

    block_on(async {
        // Pool for reusing i32 event instances to reduce allocation overhead
        let pool = OnceEventPool::<i32>::new();

        // Extract sender and receiver from an available pooled event
        let (sender, receiver) = pool.by_ref();

        println!("Created pooled event sender and receiver");

        // Send a value through the pooled event
        sender.send(EXAMPLE_INTEGER);
        println!("Sent value: {EXAMPLE_INTEGER}");

        // Receive the value
        let received = receiver.recv_async().await;
        println!("Received value: {received}");

        println!("Event automatically returned to pool when sender/receiver were dropped");

        // Reuse the pool for another event after the first one was automatically returned
        let (sender2, receiver2) = pool.by_ref();
        sender2.send(EXAMPLE_SECOND_VALUE);
        let received2 = receiver2.recv_async().await;
        println!("Second event - received value: {received2}");

        println!("Example completed successfully!");
    });
}
