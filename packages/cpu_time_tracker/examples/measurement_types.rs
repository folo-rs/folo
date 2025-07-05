//! Example demonstrating the difference between `iterations(1).thread_span()` and `iterations(1).process_span()`.
//!
//! This example shows how to use both `iterations(1).thread_span()` and `iterations(1).process_span()` methods
//! to track different types of CPU time:
//! - `iterations(1).thread_span()`: Tracks CPU time for the current thread only
//! - `iterations(1).process_span()`: Tracks CPU time for the entire process (all threads)
//!
//! Run with: `cargo run --example span_types`

use cpu_time_tracker::Session;
use std::hint::black_box;
use std::thread;

/// Performs CPU-intensive work across multiple threads.
/// This function spawns worker threads that each perform computational work.
fn multithreaded_work() {
    let num_threads = 4_u32;
    let work_per_thread = 500_000_u32;

    let handles: Vec<_> = (0..num_threads)
        .map(|thread_id| {
            thread::spawn(move || {
                let mut sum = 0_u64;

                for i in 0..work_per_thread {
                    sum = sum
                        .checked_add(
                            u64::from(i)
                                .checked_mul(u64::from(
                                    thread_id.checked_add(1).expect(
                                        "addition should not overflow for small test values",
                                    ),
                                ))
                                .expect("multiplication should not overflow for small test values")
                                % 1000,
                        )
                        .expect("addition should not overflow for small test values");
                }

                black_box(sum);
            })
        })
        .collect();

    // Do some work on the main thread while waiting
    let mut main_sum = 0_u64;

    for i in 0..500_000_u32 {
        main_sum = main_sum
            .checked_add(u64::from(i) % 1000)
            .expect("addition should not overflow for small test values");
    }

    black_box(main_sum);

    // Wait for all threads to complete
    for handle in handles {
        handle.join().expect("thread should complete successfully");
    }
}

fn main() {
    println!("=== CPU Time Measurement Types Example ===\n");

    let mut session = Session::new();

    // Example 1: Using iterations(1).thread_span() - only measures current thread's CPU time
    // Even though multithreaded_work() spawns multiple threads, iterations(1).thread_span()
    // only captures the CPU time used by the main thread (mostly coordination overhead)
    {
        let thread_op = session.operation("thread_span_multithreaded");

        for _ in 0..3 {
            let _span = thread_op.iterations(1).thread_span();
            multithreaded_work();
        }
    }

    // Example 2: Using iterations(1).process_span() - measures entire process CPU time
    // This captures CPU time from all threads spawned by multithreaded_work()
    {
        let process_op = session.operation("process_span_multithreaded");

        for _ in 0..3 {
            let _span = process_op.iterations(1).process_span();
            multithreaded_work();
        }
    }

    session.print_to_stdout();

    println!("\nNote: measure_thread should show much lower times than measure_process.");
    println!("This is because measure_thread only measures the main thread's CPU time,");
    println!("while measure_process measures CPU time from all threads in the process.");
    println!("The main thread mostly just coordinates the worker threads.");
}
