//! Integration tests for `alloc_tracker` with real memory allocations.
//!
//! These tests use a global allocator setup to test the full functionality
//! of the allocation tracking system.

#![cfg(not(miri))] // Miri replaces the global allocator, so cannot be used here.

use alloc_tracker::{Allocator, Session};

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

// Test constants to avoid magic numbers
const BYTES_PER_ITERATION: usize = 100;
const TEST_ITERATIONS: usize = 5;
const EXPECTED_TOTAL_BYTES: u64 = (BYTES_PER_ITERATION * (1 + 2 + 3 + 4 + 5)) as u64; // 1500
#[expect(clippy::integer_division, reason = "test constant calculation")]
const EXPECTED_AVERAGE_BYTES: u64 = EXPECTED_TOTAL_BYTES / TEST_ITERATIONS as u64; // 300

#[test]
fn no_span_is_empty_session() {
    let mut session = Session::new();

    _ = session.operation("test_no_span");

    assert!(session.is_empty());
}

#[test]
fn span_with_no_allocation_is_not_empty_session() {
    let mut session = Session::new();

    let op = session.operation("test_no_allocation");

    drop(op.measure_process());

    assert!(
        !session.is_empty(),
        "Session should not be empty after creating a span"
    );
}

#[test]
fn operation_allocations() {
    let mut session = Session::new();

    let average = session.operation("test_average");

    // Perform multiple allocations of different sizes
    for i in 1..=TEST_ITERATIONS {
        let _span = average.measure_process();
        let _data = vec![0_u8; i * BYTES_PER_ITERATION]; // 100, 200, 300, 400, 500 bytes
    }

    let avg = average.average();
    let iterations = average.spans();
    let total = average.total_bytes_allocated();

    assert_eq!(iterations, TEST_ITERATIONS as u64);
    assert!(
        total >= EXPECTED_TOTAL_BYTES,
        "Expected at least {EXPECTED_TOTAL_BYTES} bytes total, got {total}"
    ); // 100+200+300+400+500
    assert!(
        avg >= EXPECTED_AVERAGE_BYTES,
        "Expected average of at least {EXPECTED_AVERAGE_BYTES} bytes, got {avg}"
    ); // 1500/5
}
