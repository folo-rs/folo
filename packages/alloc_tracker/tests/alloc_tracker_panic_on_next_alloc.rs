//! Tests for the `panic_on_next_alloc` feature of the `alloc_tracker` crate.

#![cfg(feature = "panic_on_next_alloc")]

use alloc_tracker::{Allocator, panic_on_next_alloc};
use serial_test::serial;

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

#[test]
#[cfg_attr(miri, ignore)] // Test uses the real platform which cannot be executed under Miri.
#[serial]
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

#[test]
#[cfg_attr(miri, ignore)] // Test uses the real platform which cannot be executed under Miri.
#[serial]
fn panic_on_next_alloc_resets_automatically() {
    use std::panic;

    // Enable panic on next allocation
    panic_on_next_alloc(true);

    // First allocation should panic
    let result = panic::catch_unwind(|| {
        #[expect(
            clippy::useless_vec,
            reason = "we need actual allocation to test the feature"
        )]
        let _vec = vec![1, 2, 3];
    });
    assert!(result.is_err());

    // Second allocation should work because flag was reset
    #[expect(
        clippy::useless_vec,
        reason = "we need actual allocation to test the feature"
    )]
    let _allowed_allocation = vec![4, 5, 6];
}
