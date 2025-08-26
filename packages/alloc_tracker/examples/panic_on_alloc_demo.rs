//! Simple demonstration that `panic_on_next_alloc` actually works.
//!
//! This example will panic when run - it is meant to verify that the
//! panic-on-next-allocation feature is working correctly.
//!
//! To run this example, enable the `panic_on_next_alloc` feature:
//! ```bash
//! cargo run --example panic_on_alloc_demo --features panic_on_next_alloc
//! ```

#[cfg(feature = "panic_on_next_alloc")]
use alloc_tracker::{Allocator, panic_on_next_alloc};
#[cfg(not(feature = "panic_on_next_alloc"))]
use alloc_tracker::Allocator;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

#[cfg(feature = "panic_on_next_alloc")]
fn main() {
    println!("About to enable panic-on-next-allocation and then allocate...");
    panic_on_next_alloc(true);
    println!("This allocation will panic:");
    #[expect(clippy::useless_vec, reason = "example needs to show allocation")]
    let _vec = vec![1, 2, 3]; // This will panic!
    println!("This line should never be reached!");
}

#[cfg(not(feature = "panic_on_next_alloc"))]
fn main() {
    println!("This example requires the 'panic_on_next_alloc' feature to be enabled.");
    println!("Run with: cargo run --example panic_on_alloc_demo --features panic_on_next_alloc");
}
