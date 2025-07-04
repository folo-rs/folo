//! Example demonstrating the difference between `thread_span()` and `process_span()`.
//!
//! This example shows how to use both `thread_span()` and `process_span()` methods
//! to track different types of CPU time:
//! - `thread_span()`: Tracks CPU time for the current thread only
//! - `process_span()`: Tracks CPU time for the entire process (all threads)
//!
//! Run with: `cargo run --example span_types`

use cpu_time_tracker::Session;
use std::hint::black_box;

fn main() {
    println!("=== CPU Time Span Types Example ===\n");

    let mut session = Session::new();

    // Example 1: Using thread_span() for single-threaded work
    {
        let thread_op = session.operation("thread_only_work");
        for i in 0..5_u32 {
            let _span = thread_op.thread_span();
            let mut sum = 0_u64;
            for j in 0..50000_u32 {
                sum = sum
                    .checked_add(
                        u64::from(j)
                            .checked_mul(u64::from(i))
                            .expect("multiplication should not overflow for small test values")
                            % 1000,
                    )
                    .expect("addition should not overflow for small test values");
            }
            black_box(sum);
        }
    }

    // Example 2: Using process_span() for process-wide measurement
    {
        let process_op = session.operation("process_wide_work");
        for i in 0..5_u32 {
            let _span = process_op.process_span();
            let mut sum = 0_u64;
            for j in 0..50000_u32 {
                sum = sum
                    .checked_add(
                        u64::from(j)
                            .checked_mul(u64::from(i))
                            .expect("multiplication should not overflow for small test values")
                            % 1000,
                    )
                    .expect("addition should not overflow for small test values");
            }
            black_box(sum);
        }
    }

    // Example 3: Using thread_span() again (since there's no default span method)
    {
        let default_op = session.operation("default_span_work");
        for i in 0..5_u32 {
            let _span = default_op.thread_span();
            let mut sum = 0_u64;
            for j in 0..50000_u32 {
                sum = sum
                    .checked_add(
                        u64::from(j)
                            .checked_mul(u64::from(i))
                            .expect("multiplication should not overflow for small test values")
                            % 1000,
                    )
                    .expect("addition should not overflow for small test values");
            }
            black_box(sum);
        }
    }

    session.print_to_stdout();

    println!(
        "\nNote: In this single-threaded example, thread and process times should be similar."
    );
    println!("The difference becomes more apparent in multi-threaded scenarios.");
}
