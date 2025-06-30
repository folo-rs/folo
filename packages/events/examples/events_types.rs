//! Example showing how events work with different data types.
//!
//! This example demonstrates that events can work with any type, from simple
//! primitives to complex structures.

use events::OnceEvent;
use futures::executor::block_on;

/// Example task ID for demonstration purposes.
const EXAMPLE_TASK_ID: u32 = 42;

/// Example task duration in milliseconds.
const EXAMPLE_DURATION_MS: u64 = 150;

/// Example integer value for testing.
const EXAMPLE_INTEGER: i32 = -42;

#[derive(Clone, Debug)]
#[allow(
    dead_code,
    reason = "Example struct fields are only used for Debug output"
)]
struct TaskResult {
    id: u32,
    description: String,
    success: bool,
    duration_ms: u64,
}

fn main() {
    println!("=== Events Type System Example ===");

    block_on(async {
        // Example 1: Simple primitive type
        println!();
        println!("1. Integer event:");
        let int_event = OnceEvent::<i32>::new();
        let (int_sender, int_receiver) = int_event.bind_by_ref();

        int_sender.send(EXAMPLE_INTEGER);
        let int_value = int_receiver.await.expect(
            "int_sender.send() was called immediately before this, so sender cannot be dropped",
        );
        println!("Received integer: {int_value}");

        // Example 2: String type
        println!();
        println!("2. String event:");
        let string_event = OnceEvent::<String>::new();
        let (string_sender, string_receiver) = string_event.bind_by_ref();

        string_sender.send("Events support any type!".to_string());
        let string_value = string_receiver.await.expect(
            "string_sender.send() was called immediately before this, so sender cannot be dropped",
        );
        println!("Received string: {string_value}");

        // Example 3: Vector type
        println!();
        println!("3. Vector event:");
        let vec_event = OnceEvent::<Vec<u32>>::new();
        let (vec_sender, vec_receiver) = vec_event.bind_by_ref();

        vec_sender.send(vec![1, 2, 3, 4, 5]);
        let vec_value = vec_receiver.await.expect(
            "vec_sender.send() was called immediately before this, so sender cannot be dropped",
        );
        println!("Received vector: {vec_value:?}");

        // Example 4: Custom struct type
        println!();
        println!("4. Custom struct event:");
        let struct_event = OnceEvent::<TaskResult>::new();
        let (struct_sender, struct_receiver) = struct_event.bind_by_ref();

        let task = TaskResult {
            id: EXAMPLE_TASK_ID,
            description: "Process user data".to_string(),
            success: true,
            duration_ms: EXAMPLE_DURATION_MS,
        };

        struct_sender.send(task);
        let received_task = struct_receiver.await.expect(
            "struct_sender.send() was called immediately before this, so sender cannot be dropped",
        );
        println!("Received task result: {received_task:?}");

        // Example 5: Option type
        println!();
        println!("5. Option type event:");
        let option_event = OnceEvent::<Option<&str>>::new();
        let (option_sender, option_receiver) = option_event.bind_by_ref();

        option_sender.send(Some("Optional data"));
        let option_value = option_receiver.await.expect(
            "option_sender.send() was called immediately before this, so sender cannot be dropped",
        );
        println!("Received option: {option_value:?}");

        println!();
        println!("Example completed successfully!");
    });
}
