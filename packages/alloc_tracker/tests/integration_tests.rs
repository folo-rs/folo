//! Integration tests for `alloc_tracker` with real memory allocations.
//!
//! These tests use a global allocator setup to test the full functionality
//! of the allocation tracking system.

#![cfg(not(miri))] // Miri replaces the global allocator, so cannot be used here.

use std::alloc::System;

use alloc_tracker::{Allocator, Session, Span};

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

// Test constants to avoid magic numbers
const TEST_VECTOR_SIZE: usize = 1000;
const BYTES_PER_ITERATION: usize = 100;
const TEST_ITERATIONS: usize = 5;
const EXPECTED_TOTAL_BYTES: u64 = (BYTES_PER_ITERATION * (1 + 2 + 3 + 4 + 5)) as u64; // 1500
#[expect(clippy::integer_division, reason = "test constant calculation")]
const EXPECTED_AVERAGE_BYTES: u64 = EXPECTED_TOTAL_BYTES / TEST_ITERATIONS as u64; // 300
const TEST_VALUE: i32 = 42;

#[test]
fn span_with_real_allocation() {
    let session = Session::new();

    let span = Span::new(&session);

    // Allocate a vector - this should be tracked
    let data = vec![1_u8; TEST_VECTOR_SIZE];

    let delta = span.to_delta();

    // We should have allocated at least TEST_VECTOR_SIZE bytes (vector capacity might be larger)
    assert!(
        delta >= TEST_VECTOR_SIZE as u64,
        "Expected at least {TEST_VECTOR_SIZE} bytes, got {delta}"
    );

    // Keep data alive to prevent premature deallocation
    assert_eq!(data.len(), TEST_VECTOR_SIZE);
}

#[test]
fn span_no_allocation() {
    let session = Session::new();

    let span = Span::new(&session);

    // Do some work that doesn't allocate
    let x = TEST_VALUE;
    let y = x + 1;

    let delta = span.to_delta();

    // Should be zero or very close to zero
    assert_eq!(delta, 0, "Expected no allocation, got {delta} bytes");

    // Use variables to prevent optimization
    assert_eq!(y, TEST_VALUE + 1);
}

#[test]
fn average_memory_delta_with_real_allocations() {
    let mut session = Session::new();

    let average = session.operation("test_average");

    // Perform multiple allocations of different sizes
    for i in 1..=TEST_ITERATIONS {
        let _span = average.span();
        let _data = vec![0_u8; i * BYTES_PER_ITERATION]; // 100, 200, 300, 400, 500 bytes
    }

    let avg = average.average();
    let iterations = average.iterations();
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

#[test]
fn string_allocation_tracking() {
    let session = Session::new();

    let span = Span::new(&session);

    // Allocate strings
    let s1 = String::from("Hello, world!");
    let s2 = format!("Number: {TEST_VALUE}");
    let s3 = "Static string".to_string();

    let delta = span.to_delta();

    // Should have allocated memory for the strings
    assert!(
        delta > 0,
        "Expected some allocation for strings, got {delta}"
    );

    // Keep strings alive
    assert!(!s1.is_empty());
    assert!(!s2.is_empty());
    assert!(!s3.is_empty());
}

#[test]
fn boxed_allocation_tracking() {
    let session = Session::new();

    let span = Span::new(&session);

    // Allocate boxed values
    let boxed_array = Box::new([0_u64; 100]); // 800 bytes
    let boxed_vec = Box::new(vec![1_u32; 50]); // At least 200 bytes

    let delta = span.to_delta();

    // Should have allocated at least 1000 bytes
    assert!(delta >= 1000, "Expected at least 1000 bytes, got {delta}");

    // Keep boxed values alive
    assert_eq!(boxed_array.len(), 100);
    assert_eq!(boxed_vec.len(), 50);
}

#[test]
fn allocation_tracking_across_scopes() {
    let session = Session::new();

    let span = Span::new(&session);

    {
        // Allocate in inner scope
        let _temp_data = vec![0_u8; 500];

        // Even though this goes out of scope, we only track allocations, not deallocations
    }

    // Allocate more in outer scope
    let _persistent_data = vec![1_u8; 300];

    let delta = span.to_delta();

    // Should have tracked both allocations (500 + 300 = 800 bytes minimum)
    assert!(delta >= 800, "Expected at least 800 bytes, got {delta}");
}

#[test]
fn multiple_spans_independence() {
    let session = Session::new();

    let span1 = Span::new(&session);

    // Allocate some data - use vec! to ensure heap allocation
    #[allow(
        clippy::useless_vec,
        reason = "Need to allocate memory on heap for testing"
    )]
    let _data1 = vec![0_u8; 100];

    let span2 = Span::new(&session);

    // Allocate more data
    #[allow(
        clippy::useless_vec,
        reason = "Need to allocate memory on heap for testing"
    )]
    let _data2 = vec![1_u8; 200];

    let delta1 = span1.to_delta();
    let delta2 = span2.to_delta();

    // span1 should see both allocations (100 + 200 = 300 minimum)
    assert!(
        delta1 >= 300,
        "Span1 expected at least 300 bytes, got {delta1}"
    );

    // span2 should only see the second allocation (200 minimum)
    assert!(
        delta2 >= 200,
        "Span2 expected at least 200 bytes, got {delta2}"
    );
    assert!(
        delta2 < delta1,
        "Span2 should see less allocation than span1"
    );
}
