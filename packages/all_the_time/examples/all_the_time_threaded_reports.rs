//! Demonstrating thread-safe `Report` merging with `all_the_time`.
//!
//! This example shows how to use `Report` to combine processor time measurements
//! from multiple threads, including both same-operation merging and different-operation merging.
//!
//! This is not a requirement of using multiple threads but may be a useful feature in cases
//! where multiple independent session needs to be merged. The multithreading is just a simple
//! example case.
//!
//! Run with: `cargo run --example all_the_time_threaded_reports`
#![expect(
    clippy::arithmetic_side_effects,
    clippy::cast_sign_loss,
    clippy::unseparated_literal_suffix,
    reason = "this is example code that doesn't need production-level safety"
)]

use std::collections::HashMap;
use std::fmt::Write;
use std::hint::black_box;
use std::thread;

use all_the_time::{Report, Session};

fn main() {
    println!("=== Threaded Processor Time Tracking Example ===\n");

    // Create two worker threads that each do their own measurements
    let handle1 = thread::spawn(|| worker_thread("Thread-1"));
    let handle2 = thread::spawn(|| worker_thread("Thread-2"));

    // Wait for both threads and collect their reports
    let report1 = handle1
        .join()
        .expect("Thread-1 should complete successfully");
    let report2 = handle2
        .join()
        .expect("Thread-2 should complete successfully");

    println!("Report from Thread-1:");
    report1.print_to_stdout();
    println!();

    println!("Report from Thread-2:");
    report2.print_to_stdout();
    println!();

    // Merge the reports to show combined statistics
    let merged_report = Report::merge(&report1, &report2);
    println!("=== Merged Report ===");
    println!("This shows combined statistics from both threads:");
    println!("- 'common_work' operations are merged (both threads recorded this)");
    println!("- 'unique_work' operations appear separately (each thread recorded different ones)");
    println!();
    merged_report.print_to_stdout();
    println!();

    println!("✓ Successfully demonstrated thread-safe report merging");
    println!("✓ Reports contain both same-operation merging and different-operation data");
}

/// Simulates work done by a worker thread and returns a report.
fn worker_thread(thread_name: &str) -> Report {
    println!("🧵 {thread_name} starting work...");

    let session = Session::new();

    // Each thread does some "common work" that will be merged
    // Using measure_thread() to track per-thread CPU time (avoids double-counting in multithreaded scenarios)
    {
        let common_op = session.operation("common_work");
        let iterations = 10;
        let _span = common_op.measure_thread().iterations(iterations);

        for i in 0..iterations {
            // Processor-intensive string work - do more work to get measurable CPU time
            let mut result = String::new();
            for j in 0..5000 {
                write!(
                    result,
                    "{thread_name} iteration {i}-{j} with some content that is longer to force more work. "
                )
                .unwrap();
            }
            // Add some string processing to make it more processor intensive
            let processed = result.chars().rev().collect::<String>();
            black_box(processed);
        }
    }

    // Each thread also does some unique work specific to its thread
    // Using measure_thread() to track per-thread CPU time
    let unique_work_name = format!("unique_work_{thread_name}");
    {
        let unique_op = session.operation(&unique_work_name);
        let iterations = 10;
        let _span = unique_op.measure_thread().iterations(iterations);

        for i in 0..iterations {
            // Different computational work per thread - more CPU intensive
            let mut map = HashMap::new();
            for j in 0..1000 {
                let key = format!("{thread_name}-key-{i}-{j}");
                let value = format!("{thread_name}-value-{i}-{j}");
                map.insert(key, value);
            }
            // Process the map with more CPU work
            for (key, value) in &map {
                // Add some computation to make it more CPU intensive
                let mut sum = 0u64;
                for k in 0..10 {
                    sum += (key.len() as u64 * value.len() as u64 * k as u64) % 1000;
                    // Add some more computation
                    sum = sum.wrapping_mul(1103515245).wrapping_add(12345);
                }
                black_box(sum);
            }
            black_box(map);
        }
    }

    println!("🧵 {thread_name} completed work");

    // Convert session to thread-safe report
    session.to_report()
}
