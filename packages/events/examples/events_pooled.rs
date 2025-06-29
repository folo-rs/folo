//! Example demonstrating pooled events with automatic resource management.

use events::once::EventPool;
use futures::executor::block_on;

fn main() {
    println!("=== Pooled Events Example ===");

    // Create a pool for managing i32 events
    let mut pool = EventPool::<i32>::new();

    // Create sender and receiver from the pool
    let (sender, receiver) = pool.by_ref();

    println!("Created pooled event sender and receiver");

    // Send a value through the pooled event
    sender.send(42);
    println!("Sent value: 42");

    // Receive the value
    let received = block_on(receiver.recv_async());
    println!("Received value: {received}");

    println!("Event automatically returned to pool when sender/receiver were dropped");

    // Create another event from the same pool
    let (sender2, receiver2) = pool.by_ref();
    sender2.send(100);
    let received2 = block_on(receiver2.recv_async());
    println!("Second event - received value: {received2}");

    println!("Example completed successfully!");
}
