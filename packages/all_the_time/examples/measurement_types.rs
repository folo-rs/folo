//! Example demonstrating the difference between `measure_thread()` and `measure_process()`.
//!
//! This example shows how to use both `measure_thread()` and `measure_process()` methods
//! to track different types of processor time:
//! - `measure_thread()`: Tracks processor time for the current thread only
//! - `measure_process()`: Tracks processor time for the entire process (all threads)
//!
//! Run with: `cargo run --example measurement_types`.
#![expect(
    clippy::arithmetic_side_effects,
    reason = "this is example code that does not need production-level safety"
)]

use std::hint::black_box;
use std::thread;

use all_the_time::Session;

/// Performs processor-intensive work across multiple threads.
/// This function spawns worker threads that each perform computational work.
fn multithreaded_work() {
    let num_threads = 4_u32;
    let work_per_thread = 500_000_u32;

    let handles: Vec<_> = (0..num_threads)
        .map(|thread_id| {
            thread::spawn(move || {
                let mut sum = 0_u64;

                for i in 0..work_per_thread {
                    sum += u64::from(i) * u64::from(thread_id + 1) % 1000;
                }

                black_box(sum);
            })
        })
        .collect();

    // Do some work on the main thread while waiting.
    let mut main_sum = 0_u64;

    for i in 0..500_000_u32 {
        main_sum += u64::from(i) % 1000;
    }

    black_box(main_sum);

    // Wait for all threads to complete.
    for handle in handles {
        handle.join().expect("worker threads do not panic");
    }
}

fn main() {
    println!("=== Processor Time Measurement Types Example ===");
    println!();

    let session = Session::new();

    measure_thread_time(&session);
    measure_process_time(&session);

    session.print_to_stdout();

    println!();
    println!("Note: measure_thread should show much lower times than measure_process.");
    println!("This is because measure_thread only measures the main thread's processor time,");
    println!("while measure_process measures processor time from all threads in the process.");
    println!("The main thread mostly just coordinates the worker threads.");
}

/// Measures only the current thread's processor time using `measure_thread()`.
/// Even though `multithreaded_work()` spawns multiple threads, `measure_thread()`
/// only captures the processor time used by the main thread.
fn measure_thread_time(session: &Session) {
    let thread_op = session.operation("thread_span_multithreaded");

    for _ in 0..3 {
        let _span = thread_op.measure_thread();
        multithreaded_work();
    }
}

/// Measures the entire process's processor time using `measure_process()`.
/// This captures processor time from all threads spawned by `multithreaded_work()`.
fn measure_process_time(session: &Session) {
    let process_op = session.operation("process_span_multithreaded");

    for _ in 0..3 {
        let _span = process_op.measure_process();
        multithreaded_work();
    }
}
