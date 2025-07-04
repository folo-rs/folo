//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This shows how to use the `cpu_time_tracker` crate for tracking CPU time.

use cpu_time_tracker::Session;

fn main() {
    let mut session = Session::new();

    // Track a single operation
    {
        let operation = session.operation("my_operation");
        let _span = operation.thread_span();
        // Perform some CPU-intensive work
        let mut sum = 0_u64;
        for i in 0..10000_u64 {
            sum = sum
                .checked_add(i)
                .expect("addition should not overflow for small test values");
        }
        std::hint::black_box(sum);
    }

    // Print results
    session.print_to_stdout();

    // Session automatically cleans up when dropped
}
