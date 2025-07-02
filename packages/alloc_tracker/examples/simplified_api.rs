#![allow(
    clippy::collection_is_never_read,
    clippy::arithmetic_side_effects,
    reason = "This is an example file with intentional patterns that trigger these warnings"
)]

//! Example demonstrating the simplified allocation tracking API.
//!
//! This example shows the new user-friendly API that hides the complexity
//! of setting up `tracking_allocator` and managing sessions.

use std::alloc::System;

use alloc_tracker::{Allocator, Session, TrackedOperation, TrackedOperationSet};

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

fn main() {
    // Simple setup - just create a session
    let session = Session::new();

    println!("=== Simplified Allocation Tracking API ===\n");

    // Track different operations
    let mut results = TrackedOperationSet::new();

    // Vector operations
    let vec_measurement = measure_vector_operations(&session);
    results.add(vec_measurement);

    // String operations
    let string_measurement = measure_string_operations(&session);
    results.add(string_measurement);

    // HashMap operations
    let hashmap_measurement = measure_hashmap_operations(&session);
    results.add(hashmap_measurement);

    // Display all results
    println!("Results summary:");
    println!("{results}");

    // Access individual results
    if let Some(vec_bytes) = results.get("vector_operations") {
        println!("Vector operations allocated {vec_bytes} bytes on average");
    }

    println!("\nTotal operations measured: {}", results.len());
}

fn measure_vector_operations(session: &Session) -> TrackedOperation {
    let mut measurement = TrackedOperation::new("vector_operations".to_string());

    println!("Measuring vector operations...");
    for size in [10, 100, 1000] {
        let _contributor = measurement.contribute(session);
        let _vec: Vec<u64> = (0..size).collect();
    }

    println!("  {} iterations completed", measurement.iterations());
    measurement
}

fn measure_string_operations(session: &Session) -> TrackedOperation {
    let mut measurement = TrackedOperation::new("string_operations".to_string());

    println!("Measuring string operations...");
    for i in 0..5 {
        let _contributor = measurement.contribute(session);
        let _string = format!("Test string number {i} with variable length content");
    }

    println!("  {} iterations completed", measurement.iterations());
    measurement
}

fn measure_hashmap_operations(session: &Session) -> TrackedOperation {
    let mut measurement = TrackedOperation::new("hashmap_operations".to_string());

    println!("Measuring HashMap operations...");
    for size in [5, 10, 20] {
        let _contributor = measurement.contribute(session);
        let mut map = std::collections::HashMap::new();
        for i in 0..size {
            map.insert(format!("key_{i}"), i * 2);
        }
    }

    println!("  {} iterations completed", measurement.iterations());
    measurement
}
