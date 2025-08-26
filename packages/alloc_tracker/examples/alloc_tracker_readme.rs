//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This shows how to use the `alloc_tracker` package for tracking memory allocations
//! and debugging unexpected allocations. The enhanced tracker now shows both
//! the number of bytes allocated and the count of allocations.
//!
//! To run the `panic_on_next_alloc` functionality, enable the feature:
//! ```bash
//! cargo run --example alloc_tracker_readme --features panic_on_next_alloc
//! ```

#[cfg(feature = "panic_on_next_alloc")]
use alloc_tracker::{Allocator, Session, panic_on_next_alloc};
#[cfg(not(feature = "panic_on_next_alloc"))]
use alloc_tracker::{Allocator, Session};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

#[expect(clippy::useless_vec, reason = "example needs to show allocation")]
fn main() {
    // Basic allocation tracking - now shows both bytes and allocation count
    let session = Session::new();

    // Track a single operation
    {
        let operation = session.operation("my_operation");
        let _span = operation.measure_process();
        let _data = vec![1, 2, 3, 4, 5]; // This allocates memory
    }

    // Track an operation with multiple allocations to demonstrate allocation counting
    {
        let operation = session.operation("multiple_allocations");
        let _span = operation.measure_process();
        let _vec1 = vec![1, 2, 3]; // First allocation
        let _vec2 = vec![4, 5]; // Second allocation
        let _string = String::from("Hello"); // Third allocation
    }

    // Print results - now shows both mean bytes and mean allocation count
    session.print_to_stdout();

    // Session automatically cleans up when dropped

    // Debugging unexpected allocations (only with feature enabled)
    #[cfg(feature = "panic_on_next_alloc")]
    {
        // Enable panic on next allocation
        panic_on_next_alloc(true);

        // Any allocation attempt will now panic with a descriptive message
        // let _vec = vec![1, 2, 3]; // This would panic and auto-reset the flag!

        // Disable to allow allocations again
        panic_on_next_alloc(false);
        let _vec = vec![1, 2, 3]; // This is safe
    }

    #[cfg(not(feature = "panic_on_next_alloc"))]
    {
        println!("To test panic_on_next_alloc functionality, run with:");
        println!("cargo run --example alloc_tracker_readme --features panic_on_next_alloc");
        let _vec = vec![1, 2, 3]; // Regular allocation without panic functionality
    }
}
