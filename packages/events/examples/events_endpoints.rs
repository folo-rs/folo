//! Example demonstrating different ways to obtain event endpoints.
//!
//! This example shows the methods available for getting sender and receiver
//! endpoints from an event, including the checked variants.

use events::OnceEvent;
use futures::executor::block_on;

/// Example integer value for testing.
const EXAMPLE_INTEGER: i32 = 42;

/// Example large value for testing.
const EXAMPLE_LARGE_VALUE: u64 = 12345;

fn main() {
    println!("=== Event Endpoints Example ===");

    block_on(async {
        // Method 1: Get both endpoints at once using by_ref
        println!();
        println!("1. Using by_ref() method:");
        let event1 = OnceEvent::<i32>::new();
        let (sender1, receiver1) = event1.bind_by_ref();

        sender1.send(EXAMPLE_INTEGER);
        let value1 = receiver1.await.expect("sender should not have been dropped");
        println!("Received: {value1}");

        // Method 2: Using checked variant
        println!();
        println!("2. Using by_ref_checked() method:");
        let event2 = OnceEvent::<u64>::new();

        if let Some((sender2, receiver2)) = event2.bind_by_ref_checked() {
            sender2.send(EXAMPLE_LARGE_VALUE);
            let value2 = receiver2.await.expect("sender should not have been dropped");
            println!("Received via checked method: {value2}");
        }

        // Try to get endpoints again - should return None
        if event2.bind_by_ref_checked().is_some() {
            println!("This should not print - endpoints already retrieved!");
        } else {
            println!("Correctly returned None when trying to get endpoints again");
        }

        // Method 3: Demonstrating panic behavior when trying to get endpoints twice
        println!();
        println!("3. Demonstrating panic behavior:");
        let event3 = OnceEvent::<String>::new();
        let (_sender3, _receiver3) = event3.bind_by_ref();

        // This would panic if uncommented:
        // let (_sender3_again, _receiver3_again) = event3.bind_by_ref();
        println!("Would panic if we tried to call by_ref() again on event3");

        println!();
        println!("Example completed successfully!");
    });
}
