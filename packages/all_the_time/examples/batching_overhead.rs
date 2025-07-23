//! Demonstrates the overhead reduction benefits of batching.
//!
//! This example shows the measurable difference between batched and unbatched
//! measurements for operations with significant overhead.

use std::hint::black_box;
use std::time::Instant;

use all_the_time::Session;

fn main() {
    println!("=== processor time Batching Overhead Demonstration ===");
    println!();

    let session = Session::new();

    // Test: Many individual measurements (high overhead)
    println!("Testing unbatched measurements (1000 individual spans)...");

    let start_wall_time = Instant::now();
    let unbatched_op = session.operation("unbatched_measurements");

    for _ in 0..1000 {
        let _span = unbatched_op.iterations(1).measure_thread();
        // Simple operation
        black_box(42_u64.wrapping_mul(73));
    }

    let unbatched_wall_time = start_wall_time.elapsed();

    // Test: Single batched measurement (low overhead)
    println!("Testing batched measurements (1 span covering 1000 iterations)...");

    let start_wall_time = Instant::now();
    let batched_op = session.operation("batched_measurements");

    {
        let _span = batched_op.iterations(1000).measure_thread();
        for _ in 0..1000 {
            // Same simple operation
            black_box(42_u64.wrapping_mul(73));
        }
    }

    let batched_wall_time = start_wall_time.elapsed();

    // Test: More substantial work for comparison
    let substantial_op = session.operation("substantial_work");

    {
        let _span = substantial_op.iterations(5).measure_thread();
        for _ in 0..5 {
            let mut sum = 0_u64;
            for i in 0..50000_u64 {
                sum = sum.wrapping_add(i.wrapping_mul(i).wrapping_add(i));
            }
            black_box(sum);
        }
    }

    println!();
    session.print_to_stdout();
    println!();

    println!("Wall clock times (includes measurement overhead):");
    println!("  Unbatched approach: {unbatched_wall_time:?}");
    println!("  Batched approach:   {batched_wall_time:?}");

    if unbatched_wall_time > batched_wall_time {
        let overhead_reduction = unbatched_wall_time.saturating_sub(batched_wall_time);

        let percentage = if unbatched_wall_time.as_nanos() > 0 {
            #[expect(
                clippy::cast_precision_loss,
                reason = "precision loss acceptable for percentage display"
            )]
            let reduction_ns = overhead_reduction.as_nanos() as f64;

            #[expect(
                clippy::cast_precision_loss,
                reason = "precision loss acceptable for percentage display"
            )]
            let total_ns = unbatched_wall_time.as_nanos() as f64;

            (reduction_ns / total_ns) * 100.0
        } else {
            0.0
        };

        println!("  Overhead reduction: {overhead_reduction:?} ({percentage:.1}% faster)");
    }

    println!();

    println!("Key insights:");
    println!("- Both approaches measure the same total work (1000 multiplications)");
    println!("- Batching reduces measurement overhead by taking fewer timing measurements");
    println!("- For fast operations, measurement overhead can dominate actual work time");
    println!(
        "- The 'substantial_work' operation shows processor times when overhead is negligible"
    );
}
