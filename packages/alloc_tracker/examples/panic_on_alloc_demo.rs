//! Simple demonstration that `panic_on_next_alloc` actually works.
//!
//! This example will panic when run - it is meant to verify that the
//! panic-on-next-allocation feature is working correctly.

use alloc_tracker::{Allocator, panic_on_next_alloc};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

fn main() {
    println!("Enabling panic-on-next-allocation...");
    panic_on_next_alloc(true);

    println!("About to attempt allocation - this should panic!");

    // This will panic with our custom message
    #[expect(
        clippy::useless_vec,
        reason = "we want to trigger allocation to demonstrate panic"
    )]
    let _will_panic = vec![1, 2, 3];

    // This should never be reached
    println!("This should never print!");
}
