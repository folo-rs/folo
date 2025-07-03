//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This shows how to use the `alloc_tracker` crate for tracking memory allocations.

use alloc_tracker::{Allocator, Session};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

#[expect(clippy::useless_vec, reason = "example needs to show allocation")]
fn main() {
    let mut session = Session::new();

    // Track a single operation
    {
        let operation = session.operation("my_operation");
        let _span = operation.span();
        let _data = vec![1, 2, 3, 4, 5]; // This allocates memory
    }

    // Print results
    session.print_to_stdout();

    // Session automatically cleans up when dropped
}
