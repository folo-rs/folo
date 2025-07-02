//! Example demonstrating basic usage of `alloc_tracker`.
//!
//! This example shows how to use the memory tracking utilities to measure
//! memory allocations in different scenarios using the simplified API.

use std::alloc::System;

use alloc_tracker::{Allocator, Session, Operation, Span};

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

fn main() {
    // Create a tracking session (automatically handles setup)
    let session = Session::new();

    println!("=== Allocation Tracker Example (Simplified API) ===\n");

    // Example 1: Track a single operation
    println!("1. Tracking a single vector allocation:");
    single_operation_example(&session);

    println!();

    // Example 2: Track multiple operations and get an average
    println!("2. Tracking multiple operations for average:");
    average_operations_example(&session);

    println!();

    // Example 3: Compare different data structures
    println!("3. Comparing allocation patterns of different operations:");
    comparison_example(&session);

    // Session automatically cleans up when dropped
    println!("\nDone!");
}

fn single_operation_example(session: &Session) {
    let tracker = Span::new(session);

    // Allocate a vector with 1000 elements
    let data = vec![42_u64; 1000];

    let allocated_bytes = tracker.to_delta();
    println!("  Vector of 1000 u64s allocated: {allocated_bytes} bytes");
    println!("  Expected minimum: {} bytes (8 * 1000)", 8 * 1000);

    // Keep data alive to show it's still allocated
    println!("  Vector length: {}", data.len());
}

fn average_operations_example(session: &Session) {
    let mut average = Operation::new("string_formatting".to_string());

    // Perform multiple string allocations
    for i in 1..=5 {
        let _contributor = average.span(session);
        let text = format!("This is string number {i} with some content");
        println!("  Created string: \"{text}\"");
    }

    println!(
        "  Average allocation per string: {} bytes",
        average.average()
    );
    println!("  Operation name: {}", average.name());
    println!("  Total iterations: {}", average.iterations());
    println!(
        "  Total bytes allocated: {}",
        average.total_bytes_allocated()
    );
}

fn comparison_example(session: &Session) {
    // Compare Vec vs String allocations

    // Vector allocation
    let vec_tracker = Span::new(session);
    let _vec_data = vec![1_u32; 100];
    let vec_bytes = vec_tracker.to_delta();

    // String allocation
    let string_tracker = Span::new(session);
    let _string_data = "A".repeat(100);
    let string_bytes = string_tracker.to_delta();

    // Box allocation
    let box_tracker = Span::new(session);
    let _box_data = Box::new([0_u8; 100]);
    let box_bytes = box_tracker.to_delta();

    println!("  Vec<u32> with 100 elements: {vec_bytes} bytes");
    println!("  String with 100 characters: {string_bytes} bytes");
    println!("  Box<[u8; 100]>: {box_bytes} bytes");

    // Show ratios
    if string_bytes > 0 && box_bytes > 0 {
        #[allow(
            clippy::cast_precision_loss,
            reason = "Conversion for ratio display is acceptable"
        )]
        let vec_string_ratio = vec_bytes as f64 / string_bytes as f64;

        #[allow(
            clippy::cast_precision_loss,
            reason = "Conversion for ratio display is acceptable"
        )]
        let vec_box_ratio = vec_bytes as f64 / box_bytes as f64;

        println!("  Vec vs String ratio: {vec_string_ratio:.2}");
        println!("  Vec vs Box ratio: {vec_box_ratio:.2}");
    }
}
