//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This shows how to use the `cpu_time_tracker` crate for tracking CPU time.

use cpu_time_tracker::Session;

#[expect(clippy::useless_vec, reason = "example needs to show CPU work")]
fn main() {
    let mut session = Session::new();

    // Track a single operation
    {
        let operation = session.operation("my_operation");
        let _span = operation.thread_span();
        // Perform some CPU-intensive work
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
