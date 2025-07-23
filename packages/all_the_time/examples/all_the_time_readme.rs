//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This shows how to use the `all_the_time` package for tracking processor time
//! with multiple iterations.
#![expect(
    clippy::arithmetic_side_effects,
    clippy::unseparated_literal_suffix,
    reason = "this is example code that doesn't need production-level safety"
)]

use all_the_time::Session;

fn main() {
    let session = Session::new();

    // Track multiple iterations efficiently
    {
        let operation = session.operation("my_operation");
        let iterations = 10;
        let _span = operation.iterations(iterations).measure_thread();

        for i in 0u64..iterations {
            // More processor-intensive work to get measurable CPU time
            let mut sum = 0u64;
            for j in 0u64..50000 {
                for k in 0u64..10 {
                    sum += (j * k * i) % 1000;
                    // Add some more computation to make it heavier
                    sum = sum.wrapping_mul(1103515245).wrapping_add(12345);
                }
            }
            std::hint::black_box(sum);
        }
    } // Total time measured once and divided by iteration count for mean

    session.print_to_stdout();
}
