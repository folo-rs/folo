//! Example demonstrating the difference between `measure_thread()` and `measure_process()`.
//!
//! This example shows how to use both `measure_thread()` and `measure_process()` methods
//! to track different types of memory allocations:
//! - `measure_thread()`: Tracks allocations for the current thread only
//! - `measure_process()`: Tracks allocations for the entire process (all threads)
//!
//! Run with: `cargo run --example alloc_tracker_thread_vs_process`

use std::hint::black_box;
use std::thread;

use alloc_tracker::{Allocator, Session};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

/// Performs allocation-intensive work across multiple threads.
/// This function spawns worker threads that each perform memory allocations.
fn multithreaded_allocations() {
    let num_threads = 4_u32;
    let allocations_per_thread = 100_u32;

    let handles: Vec<_> = (0..num_threads)
        .map(|thread_id| {
            thread::spawn(move || {
                // Each thread allocates vectors of different sizes
                for i in 0..allocations_per_thread {
                    let size = thread_id
                        .checked_add(1)
                        .expect("addition should not overflow for small test values")
                        .checked_mul(100)
                        .expect("multiplication should not overflow for small test values")
                        .checked_add(i)
                        .expect("addition should not overflow for small test values")
                        as usize;
                    let data = vec![42_u8; size];
                    black_box(data);
                }
            })
        })
        .collect();

    // Do some allocations on the main thread while waiting
    for i in 0..50_u32 {
        let data = vec![i; 200];
        black_box(data);
    }

    // Wait for all threads to complete
    for handle in handles {
        handle.join().expect("thread should complete successfully");
    }
}

fn main() {
    println!("=== Allocation Tracking Span Types Example ===\n");

    let mut session = Session::new();

    // Example 1: Using measure_thread() - only measures current thread's allocations
    // Even though multithreaded_allocations() spawns multiple threads, measure_thread()
    // only captures the memory allocated by the main thread
    {
        let thread_op = session.operation("thread_span_multithreaded");
        let _span = thread_op.iterations(3).measure_thread();
        for _ in 0..3 {
            multithreaded_allocations();
        }
    }

    // Example 2: Using measure_process() - measures entire process allocations
    // This captures memory allocations from all threads spawned by multithreaded_allocations()
    {
        let process_op = session.operation("process_span_multithreaded");
        let _span = process_op.iterations(3).measure_process();
        for _ in 0..3 {
            multithreaded_allocations();
        }
    }

    session.print_to_stdout();

    println!(
        "\nNote: measure_thread should show much lower allocation counts than measure_process."
    );
    println!("This is because measure_thread only measures the main thread's allocations,");
    println!("while measure_process measures allocations from all threads in the process.");
    println!("The main thread only allocates a small amount compared to the worker threads.");
}
