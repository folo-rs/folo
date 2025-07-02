#![allow(
    clippy::items_after_statements,
    clippy::uninlined_format_args,
    clippy::arithmetic_side_effects,
    clippy::cast_precision_loss,
    reason = "This is an example file with intentional patterns that trigger these warnings"
)]

//! Example demonstrating how to use `alloc_tracker` in benchmark-style scenarios.
//!
//! This example shows patterns commonly used in performance testing where you
//! want to measure both execution time and memory allocation.

use std::alloc::System;
use std::time::Instant;

use alloc_tracker::{Allocator, Session, Operation};

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

fn main() {
    // Set up the allocation tracker
    let session = Session::new();

    println!("=== Benchmark-Style Memory Tracking Example ===\n");

    // Benchmark different string operations
    benchmark_string_operations(&session);

    println!();

    // Benchmark collection operations
    benchmark_collection_operations(&session);

    // Session cleanup happens automatically when session is dropped
    println!("\nBenchmarking complete!");
}

fn benchmark_string_operations(session: &Session) {
    println!("Benchmarking string operations:");

    const ITERATIONS: usize = 1000;

    // Test string concatenation with format!
    let mut format_average = Operation::new("format_strings".to_string());
    let format_start = Instant::now();

    for i in 0..ITERATIONS {
        let _contributor = format_average.span(session);
        let _result = format!("Item number {}", i);
    }

    let format_duration = format_start.elapsed();

    // Test string concatenation with String::push_str
    let mut push_str_average = Operation::new("push_str_strings".to_string());
    let push_str_start = Instant::now();

    for i in 0..ITERATIONS {
        let _contributor = push_str_average.span(session);
        let mut result = String::from("Item number ");
        result.push_str(&i.to_string());
        drop(result);
    }

    let push_str_duration = push_str_start.elapsed();

    // Test string concatenation with + operator
    let mut concat_average = Operation::new("concat_strings".to_string());
    let concat_start = Instant::now();

    for i in 0..ITERATIONS {
        let _contributor = concat_average.span(session);
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

fn benchmark_collection_operations(session: &Session) {
    println!("Benchmarking collection operations:");

    const ITERATIONS: usize = 100;
    const COLLECTION_SIZE: usize = 1000;

    // Test Vec::new() + push
    let mut vec_push_average = Operation::new("vec_push".to_string());
    let vec_push_start = Instant::now();

    for _ in 0..ITERATIONS {
        let _contributor = vec_push_average.span(session);
        let mut vec = Vec::new();
        for i in 0..COLLECTION_SIZE {
            vec.push(i);
        }
        drop(vec);
    }

    let vec_push_duration = vec_push_start.elapsed();

    // Test Vec::with_capacity + push
    let mut vec_capacity_average = Operation::new("vec_capacity".to_string());
    let vec_capacity_start = Instant::now();

    for _ in 0..ITERATIONS {
        let _contributor = vec_capacity_average.span(session);
        let mut vec = Vec::with_capacity(COLLECTION_SIZE);
        for i in 0..COLLECTION_SIZE {
            vec.push(i);
        }
        drop(vec);
    }

    let vec_capacity_duration = vec_capacity_start.elapsed();

    // Test vec! macro
    let mut vec_macro_average = Operation::new("vec_macro".to_string());
    let vec_macro_start = Instant::now();

    for _ in 0..ITERATIONS {
        let _contributor = vec_macro_average.span(session);
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
