//! Example demonstrating basic usage of allocation_tracker.
//!
//! This example shows how to use the memory tracking utilities to measure
//! memory allocations in different scenarios.

use std::alloc::System;

use allocation_tracker::{
    AverageMemoryDelta, MemoryDeltaTracker, MemoryTracker, reset_allocation_counter,
};
use tracking_allocator::{AllocationRegistry, Allocator};

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

fn main() {
    // Set up the allocation tracker
    AllocationRegistry::set_global_tracker(MemoryTracker).unwrap();
    AllocationRegistry::enable_tracking();
    reset_allocation_counter();

    println!("=== Allocation Tracker Example ===\n");

    // Example 1: Track a single operation
    println!("1. Tracking a single vector allocation:");
    single_operation_example();

    println!();

    // Example 2: Track multiple operations and get an average
    println!("2. Tracking multiple operations for average:");
    average_operations_example();

    println!();

    // Example 3: Compare different data structures
    println!("3. Comparing allocation patterns of different operations:");
    comparison_example();

    // Clean up
    AllocationRegistry::disable_tracking();
    println!("\nDone!");
}

fn single_operation_example() {
    reset_allocation_counter();
    let tracker = MemoryDeltaTracker::new();

    // Allocate a vector with 1000 elements
    let data = vec![42u64; 1000];

    let allocated_bytes = tracker.to_delta();
    println!("  Vector of 1000 u64s allocated: {} bytes", allocated_bytes);
    println!("  Expected minimum: {} bytes (8 * 1000)", 8 * 1000);

    // Keep data alive to show it's still allocated
    println!("  Vector length: {}", data.len());
}

fn average_operations_example() {
    reset_allocation_counter();
    let mut average = AverageMemoryDelta::new();

    // Perform multiple string allocations
    for i in 1..=5 {
        let _contributor = average.contribute();
        let text = format!("This is string number {} with some content", i);
        println!("  Created string: \"{}\"", text);
    }

    println!(
        "  Average allocation per string: {} bytes",
        average.average()
    );
    println!("  Total iterations: {}", average.iterations());
    println!(
        "  Total bytes allocated: {}",
        average.total_bytes_allocated()
    );
}

fn comparison_example() {
    // Compare Vec vs String allocations

    // Vector allocation
    reset_allocation_counter();
    let vec_tracker = MemoryDeltaTracker::new();
    let _vec_data = vec![1u32; 100];
    let vec_bytes = vec_tracker.to_delta();

    // String allocation
    reset_allocation_counter();
    let string_tracker = MemoryDeltaTracker::new();
    let _string_data = "A".repeat(100);
    let string_bytes = string_tracker.to_delta();

    // Box allocation
    reset_allocation_counter();
    let box_tracker = MemoryDeltaTracker::new();
    let _box_data = Box::new([0u8; 100]);
    let box_bytes = box_tracker.to_delta();

    println!("  Vec<u32> with 100 elements: {} bytes", vec_bytes);
    println!("  String with 100 characters: {} bytes", string_bytes);
    println!("  Box<[u8; 100]>: {} bytes", box_bytes);

    // Show ratios
    println!(
        "  Vec vs String ratio: {:.2}",
        vec_bytes as f64 / string_bytes as f64
    );
    println!(
        "  Vec vs Box ratio: {:.2}",
        vec_bytes as f64 / box_bytes as f64
    );
}
