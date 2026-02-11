//! Demonstrating thread-safe `Report` merging with `alloc_tracker`.
//!
//! This example shows how to use `Report` to combine memory allocation measurements
//! from multiple threads, including both same-operation merging and different-operation merging.
//!
//! This is not a requirement of using multiple threads but may be a useful feature in cases
//! where multiple independent session needs to be merged. The multithreading is just a simple
//! example case.
//!
//! Run with: `cargo run --example alloc_tracker_threaded_reports`.

use std::collections::HashMap;
use std::hint::black_box;
use std::thread;

use alloc_tracker::{Allocator, Report, Session};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn main() {
    println!("=== Threaded Allocation Tracking Example ===\n");

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
    // Using measure_thread() to track per-thread allocation (avoids double-counting in multithreaded scenarios)
    {
        let common_op = session.operation("common_work");

        // Do 3 iterations of common work in a batch
        let _span = common_op.measure_thread().iterations(3);
        for i in 0..3 {
            // Allocate strings of different sizes
            let data = format!("{thread_name} iteration {i}: ");
            let mut strings = Vec::new();
            for j in 0..50 {
                strings.push(format!("{data} string {j}"));
            }
            black_box(strings);
        }
    }

    // Each thread also does some unique work specific to its thread
    // Using measure_thread() to track per-thread allocation
    let unique_work_name = format!("unique_work_{thread_name}");
    {
        let unique_op = session.operation(&unique_work_name);

        // Do 2 iterations of unique work in a batch
        let _span = unique_op.measure_thread().iterations(2);
        for i in 0_u32..2 {
            // Different allocation patterns per thread
            if thread_name == "Thread-1" {
                // Thread 1 allocates vectors
                let mut vecs = Vec::new();
                for j in 0_u32..20 {
                    vecs.push(vec![
                        i,
                        j,
                        i.checked_add(j)
                            .expect("safe arithmetic: small loop indices cannot overflow"),
                    ]);
                }
                black_box(vecs);
            } else {
                // Thread 2 allocates hashmaps
                let mut map = HashMap::new();
                for j in 0_u32..30 {
                    map.insert(
                        format!("{thread_name}-{i}-{j}"),
                        i.checked_add(j)
                            .expect("safe arithmetic: small loop indices cannot overflow"),
                    );
                }
                black_box(map);
            }
        }
    }

    println!("🧵 {thread_name} completed work");

    // Convert session to thread-safe report
    session.to_report()
}
