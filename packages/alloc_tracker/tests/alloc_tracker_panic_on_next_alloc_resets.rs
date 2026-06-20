//! Verifies that `panic_on_next_alloc` panics on the next allocation and then resets.
//!
//! This lives in its own test binary because `panic_on_next_alloc` flips a
//! process-global flag in the global allocator: any allocation on any thread in the
//! process consumes it. Running it as the only test in its process guarantees that the
//! allocation which trips the flag is this test's own, not another test thread's.

#![cfg(feature = "panic_on_next_alloc")]

use std::panic;

use alloc_tracker::{Allocator, panic_on_next_alloc};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

#[test]
#[cfg_attr(miri, ignore)] // Test uses the real platform which cannot be executed under Miri.
fn panic_on_next_alloc_resets_automatically() {
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
