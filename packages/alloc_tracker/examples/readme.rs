//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This shows how to use the `alloc_tracker` crate for tracking memory allocations.

use alloc_tracker::{Allocator, Session, Span};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn main() {
    println!("=== Alloc Tracker README Example ===");

    let session = Session::new();

    // Track a single operation
    {
        let span = Span::new(&session);
        let data = vec![1, 2, 3, 4, 5]; // This allocates memory
        let delta = span.to_delta();
        println!("Allocated {delta} bytes");

        // Keep data alive to prevent early deallocation
        std::hint::black_box(data);
    }

    // Session automatically cleans up when dropped
    println!("README example completed successfully!");
}
