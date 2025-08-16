//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This shows how to use the `alloc_tracker` package for tracking memory allocations
//! and debugging unexpected allocations.

use alloc_tracker::{Allocator, Session, panic_on_next_alloc};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

#[expect(clippy::useless_vec, reason = "example needs to show allocation")]
fn main() {
    // Basic allocation tracking
    let session = Session::new();

    // Track a single operation
    {
        let operation = session.operation("my_operation");
        let _span = operation.measure_process();
        let _data = vec![1, 2, 3, 4, 5]; // This allocates memory
    }

    // Print results
    session.print_to_stdout();

    // Session automatically cleans up when dropped

    // Debugging unexpected allocations
    // Enable panic on next allocation
    panic_on_next_alloc(true);

    // Any allocation attempt will now panic with a descriptive message
    // let _vec = vec![1, 2, 3]; // This would panic and auto-reset the flag!

    // Disable to allow allocations again
    panic_on_next_alloc(false);
    let _vec = vec![1, 2, 3]; // This is safe
}
