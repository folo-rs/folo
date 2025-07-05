//! Demonstrates batching functionality for reducing measurement overhead.
//!
//! This example shows how to use the new API with explicit iteration counts
//! to measure many iterations of fast operations with minimal overhead.

use cpu_time_tracker::Session;
use std::hint::black_box;

fn main() {
    println!("=== CPU Time Batching Example ===");

    let mut session = Session::new();

    // Example 1: Fast operation without batching (high overhead)
    let fast_op_unbatched = session.operation("fast_operation_unbatched");
    for _ in 0..1000 {
        let _span = fast_op_unbatched.iterations(1).measure_thread();
        // Very fast operation
        black_box(42 * 2);
    }

    // Example 2: Fast operation with batching (low overhead)
    let fast_op_batched = session.operation("fast_operation_batched");
    {
        let _span = fast_op_batched.iterations(1000).measure_thread();
        for _ in 0..1000 {
            // Same very fast operation
            black_box(42 * 2);
        }
    }

    // Example 3: Medium operation for comparison
    let medium_op = session.operation("medium_operation");
    {
        let _span = medium_op.iterations(10).measure_thread();
        for _ in 0..10 {
            // Medium-sized operation
            let mut sum = 0_u64;
            for i in 0..10000_u64 {
                sum = sum.wrapping_add(i.wrapping_mul(i));
            }
            black_box(sum);
        }
    }

    // Example 4: Process-level batching
    let process_batched = session.operation("process_operation_batched");
    {
        let _span = process_batched.iterations(500).measure_process();
        for _ in 0..500 {
            // Fast operation measured at process level
            black_box(std::ptr::null::<i32>() as usize);
        }
    }

    session.print_to_stdout();
    println!();

    println!("Notes:");
    println!(
        "- 'fast_operation_unbatched' likely shows higher per-operation times due to measurement overhead"
    );
    println!(
        "- 'fast_operation_batched' should show lower per-operation times with the same total work"
    );
    println!(
        "- Both batched operations measured 1000/500 iterations but only took 1 measurement each"
    );
    println!("- For very fast operations, batching can significantly reduce measurement overhead");
}
