//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This shows how to use the `all_the_time` crate for tracking processor time.
#![expect(
    clippy::arithmetic_side_effects,
    reason = "this is example code that doesn't need production-level safety"
)]

use all_the_time::Session;

fn main() {
    let mut session = Session::new();

    // Track a single operation
    {
        let operation = session.operation("my_operation");
        let _span = operation.iterations(1).measure_thread();

        // Perform some processor-intensive work
        let mut sum = 0;
        for i in 0..10000 {
            sum += i;
        }

        std::hint::black_box(sum);
    }

    // Print results
    session.print_to_stdout();

    // Session automatically cleans up when dropped
}
