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
    // The flag is armed inside the closure, immediately before the allocation that
    // is expected to consume it. Because the one-shot flag is consumed by the very
    // next allocation anywhere in the process, arming it earlier would let an
    // allocation performed by `catch_unwind`'s own setup consume it first, making
    // the panic escape the caught region. Instrumented standard libraries (for
    // example under `cargo careful`) are prone to such incidental allocations.
    let result = panic::catch_unwind(|| {
        panic_on_next_alloc(true);
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
