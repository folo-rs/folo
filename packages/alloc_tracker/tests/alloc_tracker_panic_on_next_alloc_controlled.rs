//! Verifies that `panic_on_next_alloc` can be toggled on and off without panicking.
//!
//! This lives in its own test binary because `panic_on_next_alloc` flips a
//! process-global flag in the global allocator: any allocation on any thread in the
//! process consumes it. Running it as the only test in its process guarantees no other
//! test thread can trip the flag while this test holds it.

#![cfg(feature = "panic_on_next_alloc")]

use alloc_tracker::{Allocator, panic_on_next_alloc};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

#[test]
#[cfg_attr(miri, ignore)] // Test uses the real platform which cannot be executed under Miri.
fn panic_on_next_alloc_can_be_controlled() {
    // This test verifies the API works but does not test the panic behavior
    // as that would terminate the test process

    // Default state should allow allocations
    panic_on_next_alloc(false);
    #[expect(
        clippy::useless_vec,
        reason = "we need actual allocation to test the feature"
    )]
    let _allowed_allocation = vec![1, 2, 3];

    // Enable and then immediately disable panic on next allocation
    // We do not test the actual panic as it would kill the test
    panic_on_next_alloc(true);
    panic_on_next_alloc(false);

    // Allocations should work again
    #[expect(
        clippy::useless_vec,
        reason = "we need actual allocation to test the feature"
    )]
    let _another_allowed_allocation = vec![4, 5, 6];
}
