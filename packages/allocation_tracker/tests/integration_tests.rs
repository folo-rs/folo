//! Integration tests for `allocation_tracker` with real memory allocations.
//!
//! These tests use a global allocator setup to test the full functionality
//! of the allocation tracking system.

use std::alloc::System;

use allocation_tracker::{
    AllocationTrackingSession, AverageMemoryDelta, MemoryDeltaTracker, TrackingAllocator,
};

#[global_allocator]
static ALLOCATOR: TrackingAllocator<System> = TrackingAllocator::system();

fn setup_tracking() -> AllocationTrackingSession {
    AllocationTrackingSession::new()
}

fn cleanup_tracking() {
    // Session cleanup happens automatically when session is dropped
}

#[test]
fn memory_delta_tracker_with_real_allocation() {
    let session = setup_tracking();

    let tracker = MemoryDeltaTracker::new(&session);

    // Allocate a vector - this should be tracked
    let data = vec![1_u8; 1000];

    let delta = tracker.to_delta();

    // We should have allocated at least 1000 bytes (vector capacity might be larger)
    assert!(delta >= 1000, "Expected at least 1000 bytes, got {delta}");

    // Keep data alive to prevent premature deallocation
    assert_eq!(data.len(), 1000);

    cleanup_tracking();
}

#[test]
fn memory_delta_tracker_no_allocation() {
    let session = setup_tracking();

    let tracker = MemoryDeltaTracker::new(&session);

    // Do some work that doesn't allocate
    let x = 42;
    let y = x + 1;

    let delta = tracker.to_delta();

    // Should be zero or very close to zero
    assert_eq!(delta, 0, "Expected no allocation, got {delta} bytes");

    // Use variables to prevent optimization
    assert_eq!(y, 43);

    cleanup_tracking();
}

#[test]
fn average_memory_delta_with_real_allocations() {
    let session = setup_tracking();

    let mut average = AverageMemoryDelta::new("test_average".to_string());

    // Perform multiple allocations of different sizes
    for i in 1..=5 {
        let _contributor = average.contribute(&session);
        let _data = vec![0_u8; i * 100]; // 100, 200, 300, 400, 500 bytes
    }

    let avg = average.average();
    let iterations = average.iterations();
    let total = average.total_bytes_allocated();

    assert_eq!(iterations, 5);
    assert!(
        total >= 1500,
        "Expected at least 1500 bytes total, got {total}"
    ); // 100+200+300+400+500
    assert!(
        avg >= 300,
        "Expected average of at least 300 bytes, got {avg}"
    ); // 1500/5

    cleanup_tracking();
}

#[test]
fn string_allocation_tracking() {
    let session = setup_tracking();

    let tracker = MemoryDeltaTracker::new(&session);

    // Allocate strings
    let s1 = String::from("Hello, world!");
    let s2 = format!("Number: {}", 42);
    let s3 = "Static string".to_string();

    let delta = tracker.to_delta();

    // Should have allocated memory for the strings
    assert!(
        delta > 0,
        "Expected some allocation for strings, got {delta}"
    );

    // Keep strings alive
    assert!(!s1.is_empty());
    assert!(!s2.is_empty());
    assert!(!s3.is_empty());

    cleanup_tracking();
}

#[test]
fn boxed_allocation_tracking() {
    let session = setup_tracking();

    let tracker = MemoryDeltaTracker::new(&session);

    // Allocate boxed values
    let boxed_array = Box::new([0_u64; 100]); // 800 bytes
    let boxed_vec = Box::new(vec![1_u32; 50]); // At least 200 bytes

    let delta = tracker.to_delta();

    // Should have allocated at least 1000 bytes
    assert!(delta >= 1000, "Expected at least 1000 bytes, got {delta}");

    // Keep boxed values alive
    assert_eq!(boxed_array.len(), 100);
    assert_eq!(boxed_vec.len(), 50);

    cleanup_tracking();
}

#[test]
fn allocation_tracking_across_scopes() {
    let session = setup_tracking();

    let tracker = MemoryDeltaTracker::new(&session);

    {
        // Allocate in inner scope
        let _temp_data = vec![0_u8; 500];

        // Even though this goes out of scope, we only track allocations, not deallocations
    }

    // Allocate more in outer scope
    let _persistent_data = vec![1_u8; 300];

    let delta = tracker.to_delta();

    // Should have tracked both allocations (500 + 300 = 800 bytes minimum)
    assert!(delta >= 800, "Expected at least 800 bytes, got {delta}");

    cleanup_tracking();
}

#[test]
fn multiple_trackers_independence() {
    let session = setup_tracking();

    let tracker1 = MemoryDeltaTracker::new(&session);

    // Allocate some data - use vec! to ensure heap allocation
    #[allow(
        clippy::useless_vec,
        reason = "Need to allocate memory on heap for testing"
    )]
    let _data1 = vec![0_u8; 100];

    let tracker2 = MemoryDeltaTracker::new(&session);

    // Allocate more data
    #[allow(
        clippy::useless_vec,
        reason = "Need to allocate memory on heap for testing"
    )]
    let _data2 = vec![1_u8; 200];

    let delta1 = tracker1.to_delta();
    let delta2 = tracker2.to_delta();

    // tracker1 should see both allocations (100 + 200 = 300 minimum)
    assert!(
        delta1 >= 300,
        "Tracker1 expected at least 300 bytes, got {delta1}"
    );

    // tracker2 should only see the second allocation (200 minimum)
    assert!(
        delta2 >= 200,
        "Tracker2 expected at least 200 bytes, got {delta2}"
    );
    assert!(
        delta2 < delta1,
        "Tracker2 should see less allocation than tracker1"
    );

    cleanup_tracking();
}
