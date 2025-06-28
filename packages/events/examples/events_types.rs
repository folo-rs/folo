//! Example showing how events work with different data types.
//!
//! This example demonstrates that events can work with any type, from simple
//! primitives to complex structures.

use events::once::Event;

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

    // Example 1: Simple primitive type
    println!("\n1. Integer event:");
    let int_event = Event::<i32>::new();
    let (int_sender, int_receiver) = int_event.by_ref();

    int_sender.send(-42);
    let int_value = int_receiver.receive();
    println!("Received integer: {int_value}");

    // Example 2: String type
    println!("\n2. String event:");
    let string_event = Event::<String>::new();
    let (string_sender, string_receiver) = string_event.by_ref();

    string_sender.send("Events support any type!".to_string());
    let string_value = string_receiver.receive();
    println!("Received string: {string_value}");

    // Example 3: Vector type
    println!("\n3. Vector event:");
    let vec_event = Event::<Vec<u32>>::new();
    let (vec_sender, vec_receiver) = vec_event.by_ref();

    vec_sender.send(vec![1, 2, 3, 4, 5]);
    let vec_value = vec_receiver.receive();
    println!("Received vector: {vec_value:?}");

    // Example 4: Custom struct type
    println!("\n4. Custom struct event:");
    let struct_event = Event::<TaskResult>::new();
    let (struct_sender, struct_receiver) = struct_event.by_ref();

    let task = TaskResult {
        id: 42,
        description: "Process user data".to_string(),
        success: true,
        duration_ms: 150,
    };

    struct_sender.send(task);
    let received_task = struct_receiver.receive();
    println!("Received task result: {received_task:?}");

    // Example 5: Option type
    println!("\n5. Option type event:");
    let option_event = Event::<Option<&str>>::new();
    let (option_sender, option_receiver) = option_event.by_ref();

    option_sender.send(Some("Optional data"));
    let option_value = option_receiver.receive();
    println!("Received option: {option_value:?}");

    println!("\nExample completed successfully!");
}
