//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This shows how to use the `all_the_time` package for tracking processor time
//! with multiple iterations.
#![expect(
    clippy::arithmetic_side_effects,
    reason = "this is example code that doesn't need production-level safety"
)]

use all_the_time::Session;

fn main() {
    let mut session = Session::new();

    // Track multiple iterations efficiently
    {
        let operation = session.operation("my_operation");
        let iterations = 1000;
        let _span = operation.iterations(iterations).measure_thread();

        for _ in 0..iterations {
            let mut sum = 0;
            for j in 0..100 {
                sum += j * j;
            }
            std::hint::black_box(sum);
        }
    } // Total time measured once and divided by iteration count for mean

    session.print_to_stdout();
}
