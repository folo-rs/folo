//! Example demonstrating how to use allocation_tracker in benchmark-style scenarios.
//!
//! This example shows patterns commonly used in performance testing where you
//! want to measure both execution time and memory allocation.

use std::alloc::System;
use std::time::Instant;

use allocation_tracker::{AverageMemoryDelta, MemoryTracker, reset_allocation_counter};
use tracking_allocator::{AllocationRegistry, Allocator};

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

fn main() {
    // Set up the allocation tracker
    AllocationRegistry::set_global_tracker(MemoryTracker).unwrap();
    AllocationRegistry::enable_tracking();

    println!("=== Benchmark-Style Memory Tracking Example ===\n");

    // Benchmark different string operations
    benchmark_string_operations();

    println!();

    // Benchmark collection operations
    benchmark_collection_operations();

    // Clean up
    AllocationRegistry::disable_tracking();
    println!("\nBenchmarking complete!");
}

fn benchmark_string_operations() {
    println!("Benchmarking string operations:");

    const ITERATIONS: usize = 1000;

    // Test string concatenation with format!
    let mut format_average = AverageMemoryDelta::new();
    let format_start = Instant::now();

    for i in 0..ITERATIONS {
        let _contributor = format_average.contribute();
        let _result = format!("Item number {}", i);
    }

    let format_duration = format_start.elapsed();

    // Test string concatenation with String::push_str
    reset_allocation_counter();
    let mut push_str_average = AverageMemoryDelta::new();
    let push_str_start = Instant::now();

    for i in 0..ITERATIONS {
        let _contributor = push_str_average.contribute();
        let mut result = String::from("Item number ");
        result.push_str(&i.to_string());
        drop(result);
    }

    let push_str_duration = push_str_start.elapsed();

    // Test string concatenation with + operator
    reset_allocation_counter();
    let mut concat_average = AverageMemoryDelta::new();
    let concat_start = Instant::now();

    for i in 0..ITERATIONS {
        let _contributor = concat_average.contribute();
        let _result = "Item number ".to_string() + &i.to_string();
    }

    let concat_duration = concat_start.elapsed();

    // Print results
    println!("  format! macro:");
    println!("    Time: {:.2?}", format_duration);
    println!(
        "    Average allocation: {} bytes per operation",
        format_average.average()
    );

    println!("  String::push_str:");
    println!("    Time: {:.2?}", push_str_duration);
    println!(
        "    Average allocation: {} bytes per operation",
        push_str_average.average()
    );

    println!("  String concatenation (+):");
    println!("    Time: {:.2?}", concat_duration);
    println!(
        "    Average allocation: {} bytes per operation",
        concat_average.average()
    );
}

fn benchmark_collection_operations() {
    println!("Benchmarking collection operations:");

    const ITERATIONS: usize = 100;
    const COLLECTION_SIZE: usize = 1000;

    // Test Vec::new() + push
    reset_allocation_counter();
    let mut vec_push_average = AverageMemoryDelta::new();
    let vec_push_start = Instant::now();

    for _ in 0..ITERATIONS {
        let _contributor = vec_push_average.contribute();
        let mut vec = Vec::new();
        for i in 0..COLLECTION_SIZE {
            vec.push(i);
        }
        drop(vec);
    }

    let vec_push_duration = vec_push_start.elapsed();

    // Test Vec::with_capacity + push
    reset_allocation_counter();
    let mut vec_capacity_average = AverageMemoryDelta::new();
    let vec_capacity_start = Instant::now();

    for _ in 0..ITERATIONS {
        let _contributor = vec_capacity_average.contribute();
        let mut vec = Vec::with_capacity(COLLECTION_SIZE);
        for i in 0..COLLECTION_SIZE {
            vec.push(i);
        }
        drop(vec);
    }

    let vec_capacity_duration = vec_capacity_start.elapsed();

    // Test vec! macro
    reset_allocation_counter();
    let mut vec_macro_average = AverageMemoryDelta::new();
    let vec_macro_start = Instant::now();

    for _ in 0..ITERATIONS {
        let _contributor = vec_macro_average.contribute();
        let _vec = vec![0; COLLECTION_SIZE];
    }

    let vec_macro_duration = vec_macro_start.elapsed();

    // Print results
    println!("  Vec::new() + {} pushes:", COLLECTION_SIZE);
    println!("    Time: {:.2?}", vec_push_duration);
    println!(
        "    Average allocation: {} bytes per collection",
        vec_push_average.average()
    );

    println!(
        "  Vec::with_capacity({}) + {} pushes:",
        COLLECTION_SIZE, COLLECTION_SIZE
    );
    println!("    Time: {:.2?}", vec_capacity_duration);
    println!(
        "    Average allocation: {} bytes per collection",
        vec_capacity_average.average()
    );

    println!("  vec![0; {}]:", COLLECTION_SIZE);
    println!("    Time: {:.2?}", vec_macro_duration);
    println!(
        "    Average allocation: {} bytes per collection",
        vec_macro_average.average()
    );

    // Show efficiency comparison
    println!("\n  Efficiency comparison:");
    println!(
        "    Pre-allocated vs growing: {:.2}x less allocation",
        vec_push_average.average() as f64 / vec_capacity_average.average() as f64
    );
    println!(
        "    Macro vs pre-allocated: {:.2}x allocation ratio",
        vec_macro_average.average() as f64 / vec_capacity_average.average() as f64
    );
}
