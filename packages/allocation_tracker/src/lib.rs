//! Memory allocation tracking utilities for benchmarks and performance analysis.
//!
//! This crate provides utilities to track memory allocations during code execution,
//! enabling measurement of memory usage patterns in benchmarks and performance tests.
//!
//! The core functionality includes:
//! - [`TrackingAllocator`] - A Rust memory allocator wrapper that enables allocation tracking
//! - [`AllocationTrackingSession`] - Configures allocation tracking and provides access to tracking data
//! - [`MemoryDeltaTracker`] - Tracks memory allocation changes over a time period
//! - [`AverageMemoryDelta`] - Calculates average memory allocation per operation
//! - [`MemoryUsageResults`] - Collects and displays memory usage measurements
//!
//! # Quick Start
//!
//! Add this to your `Cargo.toml`:
//!
//! ```toml
//! [dependencies]
//! allocation_tracker = "0.1.0"
//! ```
//!
//! # Simple Usage
//!
//! With the new simplified API, you can track allocations like this:
//!
//! ```
//! use allocation_tracker::{TrackingAllocator, AllocationTrackingSession, MemoryDeltaTracker};
//! use std::alloc::System;
//!
//! #[global_allocator]
//! static ALLOCATOR: TrackingAllocator<System> = TrackingAllocator::system();
//!
//! fn main() {
//!     let session = AllocationTrackingSession::new().unwrap();
//!     
//!     // Track a single operation
//!     let tracker = MemoryDeltaTracker::new(&session);
//!     let data = vec![1, 2, 3, 4, 5]; // This allocates memory
//!     let delta = tracker.to_delta();
//!     println!("Allocated {} bytes", delta);
//!
//!     // Session automatically cleans up when dropped
//! }
//! ```
//!
//! # Tracking Average Allocations
//!
//! For benchmarking scenarios, use [`AverageMemoryDelta`]:
//!
//! ```
//! use allocation_tracker::{TrackingAllocator, AllocationTrackingSession, AverageMemoryDelta};
//! use std::alloc::System;
//!
//! #[global_allocator]
//! static ALLOCATOR: TrackingAllocator<System> = TrackingAllocator::system();
//!
//! fn main() {
//!     let session = AllocationTrackingSession::new().unwrap();
//!     let mut average = AverageMemoryDelta::new("string_allocations".to_string());
//!     
//!     // Track average over multiple operations
//!     for i in 0..10 {
//!         let _contributor = average.contribute(&session);
//!         let _data = format!("String number {}", i); // This allocates memory
//!     }
//!     
//!     println!("Average allocation: {} bytes per operation", average.average());
//!     println!("Operation name: {}", average.name());
//! }
//! ```
//!
//! ## Integration with `tracking_allocator`
//!
//! For real allocation tracking, you need to set up [`TrackingAllocator`] as your global allocator.
//! This requires a binary crate (application), not a library crate:
//!
//! ```rust
//! use allocation_tracker::{TrackingAllocator, AllocationTrackingSession, MemoryDeltaTracker};
//! use std::alloc::System;
//!
//! #[global_allocator]
//! static ALLOCATOR: TrackingAllocator<System> = TrackingAllocator::system();
//!
//! fn main() {
//!     // Create a tracking session (automatically handles setup)
//!     let session = AllocationTrackingSession::new().unwrap();
//!     
//!     // Track a single operation
//!     let tracker = MemoryDeltaTracker::new(&session);
//!     let data = vec![1, 2, 3, 4, 5]; // This allocates memory
//!     let bytes_allocated = tracker.to_delta();
//!     
//!     println!("Allocated {} bytes", bytes_allocated);
//!     
//!     // Session automatically disables tracking when dropped
//! }
//! ```
//!
//! ## Collecting Multiple Measurements
//!
//! Use [`MemoryUsageResults`] to collect and display measurements from different operations:
//!
//! ```
//! use allocation_tracker::{TrackingAllocator, AllocationTrackingSession, AverageMemoryDelta, MemoryUsageResults};
//! use std::alloc::System;
//!
//! #[global_allocator]
//! static ALLOCATOR: TrackingAllocator<System> = TrackingAllocator::system();
//!
//! fn main() {
//!     let session = AllocationTrackingSession::new().unwrap();
//!     let mut results = MemoryUsageResults::new();
//!
//!     // Create and measure different operations
//!     let mut vec_measurement = AverageMemoryDelta::new("vector_creation".to_string());
//!     {
//!         let _contributor = vec_measurement.contribute(&session);
//!         let _vec = vec![1, 2, 3, 4, 5]; // This allocates memory
//!     }
//!     results.add(vec_measurement);
//!
//!     let mut string_measurement = AverageMemoryDelta::new("string_creation".to_string());
//!     {
//!         let _contributor = string_measurement.contribute(&session);
//!         let _string = String::from("Hello, world!"); // This allocates memory  
//!     }
//!     results.add(string_measurement);
//!
//!     // Print all results
//!     println!("{}", results);
//!
//!     // Access individual results
//!     if let Some(bytes) = results.get("vector_creation") {
//!         println!("Vector creation used {} bytes", bytes);
//!     }
//! }
//! ```
//!
//! ## Use in Criterion Benchmarks
//!
//! This crate integrates well with Criterion for memory-aware benchmarking:
//!
//! ```rust
//! use allocation_tracker::{TrackingAllocator, AllocationTrackingSession, AverageMemoryDelta};
//! use criterion::{criterion_group, criterion_main, Criterion};
//! use std::alloc::System;
//!
//! #[global_allocator]
//! static ALLOCATOR: TrackingAllocator<System> = TrackingAllocator::system();
//!
//! fn bench_string_allocation(c: &mut Criterion) {
//!     let session = AllocationTrackingSession::new().unwrap();
//!     
//!     let mut group = c.benchmark_group("memory_usage");
//!     
//!     group.bench_function("string_formatting", |b| {
//!         let mut average_memory_delta = AverageMemoryDelta::new("string_formatting".to_string());
//!
//!         b.iter(|| {
//!             let _contributor = average_memory_delta.contribute(&session);
//!             let s = format!("Hello, {}!", "world");
//!             std::hint::black_box(s);
//!         });
//!
//!         println!("Average allocation: {} bytes", average_memory_delta.average());
//!     });
//!     
//!     group.finish();
//! }
//!
//! criterion_group!(benches, bench_string_allocation);
//! criterion_main!(benches);
//! ```
//!
//! # Important Notes
//!
//! ## Global Allocator Requirement
//!
//! This crate requires a global allocator to be set up using [`TrackingAllocator`]. 
//! This can only be done in binary crates (applications), not library crates.
//!
//! For library crates that want to test allocation tracking:
//! - Use integration tests (in `tests/` directory) which compile as separate binaries
//! - Each integration test can set its own global allocator
//!
//! ## Testing Strategy
//!
//! - **Unit tests** - Test logic that doesn't require allocation tracking
//! - **Integration tests** - Test full functionality with real allocations  
//! - **Examples** - Demonstrate real-world usage patterns
//!
//! ## Thread Safety
//!
//! The allocation tracking is thread-safe using atomic operations. However, measurements
//! across multiple threads will be combined, so single-threaded testing is recommended
//! for precise measurements.
//!
//! ## Session Management
//!
//! Only one [`AllocationTrackingSession`] can be active at a time. Attempting to create
//! multiple sessions simultaneously will result in an error. This ensures that tracking
//! state is properly managed and measurements are accurate.

#![allow(
    missing_docs,
    reason = "API documentation is provided at the crate level for now"
)]

mod allocator;
mod average;
mod delta;
mod results;
mod session;
mod tracker;
mod utils;

pub use allocator::*;
pub use average::*;
pub use delta::*;
pub use results::*;
pub use session::*;
pub use tracker::*;
pub use utils::*;



/// Manages allocation tracking session state.
///
/// This type ensures that allocation tracking is properly enabled and disabled,
/// and prevents multiple concurrent tracking sessions which would interfere with
/// each other.
///
/// # Examples
///
/// ```rust
/// use allocation_tracker::{AllocationTrackingSession, MemoryDeltaTracker};
///
/// let session = AllocationTrackingSession::new().unwrap();
/// let tracker = MemoryDeltaTracker::new(&session);
#[cfg(test)]
mod tests {
    //! Unit tests for `allocation_tracker`.
    //!
    //! These tests focus on the logic that doesn't require the `MemoryTracker` to be
    //! wired up as a global allocator. Integration tests for the full functionality
    //! are in the tests/ directory.

    use crate::{
        AverageMemoryDelta, MemoryDeltaTracker, MemoryUsageResults, reset_allocation_counter,
        AllocationTrackingSession,
    };

    // Helper function to create a mock session for testing
    // Note: This won't actually enable allocation tracking since we're not using
    // a global allocator in unit tests, but it allows us to test the API structure
    fn create_test_session() -> AllocationTrackingSession {
        // For unit tests, we'll create a session that might fail but we'll ignore it
        // since these tests are focused on logic that doesn't require real allocation tracking
        AllocationTrackingSession::new().unwrap_or_else(|_| {
            // If session creation fails, we can still test the basic logic
            // This is a bit of a hack for unit testing
            AllocationTrackingSession { _private: () }
        })
    }

    #[test]
    fn average_memory_delta_new() {
        let average = AverageMemoryDelta::new("test".to_string());
        assert_eq!(average.name(), "test");
        assert_eq!(average.average(), 0);
        assert_eq!(average.iterations(), 0);
        assert_eq!(average.total_bytes_allocated(), 0);
    }

    #[test]
    fn average_memory_delta_add_single() {
        let mut average = AverageMemoryDelta::new("test".to_string());
        average.add(100);

        assert_eq!(average.average(), 100);
        assert_eq!(average.iterations(), 1);
        assert_eq!(average.total_bytes_allocated(), 100);
    }

    #[test]
    fn average_memory_delta_add_multiple() {
        let mut average = AverageMemoryDelta::new("test".to_string());
        average.add(100);
        average.add(200);
        average.add(300);

        assert_eq!(average.average(), 200); // (100 + 200 + 300) / 3
        assert_eq!(average.iterations(), 3);
        assert_eq!(average.total_bytes_allocated(), 600);
    }

    #[test]
    fn average_memory_delta_add_zero() {
        let mut average = AverageMemoryDelta::new("test".to_string());
        average.add(0);
        average.add(0);

        assert_eq!(average.average(), 0);
        assert_eq!(average.iterations(), 2);
        assert_eq!(average.total_bytes_allocated(), 0);
    }

    #[test]
    fn memory_delta_tracker_new() {
        reset_allocation_counter();
        let session = create_test_session();
        let tracker = MemoryDeltaTracker::new(&session);
        assert_eq!(tracker.to_delta(), 0);
    }

    #[test]
    fn memory_delta_tracker_with_simulated_allocation() {
        reset_allocation_counter();
        let session = create_test_session();
        let tracker = MemoryDeltaTracker::new(&session);

        // Simulate some allocation by manually adding to the counter
        crate::tracker::TRACKER_BYTES_ALLOCATED.fetch_add(150, std::sync::atomic::Ordering::Relaxed);

        assert_eq!(tracker.to_delta(), 150);
    }

    #[test]
    fn memory_delta_tracker_multiple_trackers() {
        reset_allocation_counter();
        let session = create_test_session();
        let tracker1 = MemoryDeltaTracker::new(&session);

        // Simulate allocation
        crate::tracker::TRACKER_BYTES_ALLOCATED.fetch_add(100, std::sync::atomic::Ordering::Relaxed);

        let tracker2 = MemoryDeltaTracker::new(&session);

        // Simulate more allocation
        crate::tracker::TRACKER_BYTES_ALLOCATED.fetch_add(50, std::sync::atomic::Ordering::Relaxed);

        assert_eq!(tracker1.to_delta(), 150); // Total since tracker1 was created
        assert_eq!(tracker2.to_delta(), 50); // Only since tracker2 was created
    }

    #[test]
    fn average_memory_delta_contributor_drop() {
        reset_allocation_counter();
        let session = create_test_session();
        let mut average = AverageMemoryDelta::new("test".to_string());

        {
            let _contributor = average.contribute(&session);
            // Simulate allocation
            crate::tracker::TRACKER_BYTES_ALLOCATED.fetch_add(75, std::sync::atomic::Ordering::Relaxed);
        } // Contributor drops here

        assert_eq!(average.average(), 75);
        assert_eq!(average.iterations(), 1);
        assert_eq!(average.total_bytes_allocated(), 75);
    }

    #[test]
    fn average_memory_delta_multiple_contributors() {
        reset_allocation_counter();
        let session = create_test_session();
        let mut average = AverageMemoryDelta::new("test".to_string());

        // First contributor
        {
            let _contributor = average.contribute(&session);
            crate::tracker::TRACKER_BYTES_ALLOCATED.fetch_add(100, std::sync::atomic::Ordering::Relaxed);
        }

        // Second contributor
        {
            let _contributor = average.contribute(&session);
            crate::tracker::TRACKER_BYTES_ALLOCATED.fetch_add(200, std::sync::atomic::Ordering::Relaxed);
        }

        assert_eq!(average.average(), 150); // (100 + 200) / 2
        assert_eq!(average.iterations(), 2);
        assert_eq!(average.total_bytes_allocated(), 300);
    }

    #[test]
    fn average_memory_delta_contributor_no_allocation() {
        reset_allocation_counter();
        let session = create_test_session();
        let mut average = AverageMemoryDelta::new("test".to_string());

        {
            let _contributor = average.contribute(&session);
            // No allocation
        }

        assert_eq!(average.average(), 0);
        assert_eq!(average.iterations(), 1);
        assert_eq!(average.total_bytes_allocated(), 0);
    }

    #[test]
    fn reset_and_current_allocation_count() {
        // Set some value
        crate::tracker::TRACKER_BYTES_ALLOCATED.store(500, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(crate::current_allocation_count(), 500);

        // Reset to zero
        reset_allocation_counter();
        assert_eq!(crate::current_allocation_count(), 0);
    }

    /// This test ensures our internal access to TRACKER_BYTES_ALLOCATED works correctly.
    #[test]
    fn internal_counter_access() {
        reset_allocation_counter();
        assert_eq!(crate::current_allocation_count(), 0);

        crate::tracker::TRACKER_BYTES_ALLOCATED.fetch_add(42, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(crate::current_allocation_count(), 42);

        reset_allocation_counter();
        assert_eq!(crate::current_allocation_count(), 0);
    }

    #[test]
    fn memory_usage_results_new() {
        let results = MemoryUsageResults::new();
        assert_eq!(results.len(), 0);
        assert!(results.is_empty());
    }

    #[test]
    fn memory_usage_results_add_and_get() {
        let mut results = MemoryUsageResults::new();
        let measurement = AverageMemoryDelta::new("test_op".to_string());

        results.add(measurement);
        assert_eq!(results.len(), 1);
        assert!(!results.is_empty());
        assert_eq!(results.get("test_op"), Some(0)); // No actual allocations in unit test
        assert_eq!(results.get("nonexistent"), None);
    }

    #[test]
    fn memory_usage_results_add_explicit() {
        let mut results = MemoryUsageResults::new();

        results.add_explicit("test_op".to_string(), 42);
        assert_eq!(results.len(), 1);
        assert!(!results.is_empty());
        assert_eq!(results.get("test_op"), Some(42));
        assert_eq!(results.get("nonexistent"), None);
    }

    #[test]
    fn memory_usage_results_multiple_operations() {
        let mut results = MemoryUsageResults::new();

        results.add_explicit("op1".to_string(), 100);
        results.add_explicit("op2".to_string(), 200);
        results.add_explicit("op3".to_string(), 300);

        assert_eq!(results.len(), 3);
        assert_eq!(results.get("op1"), Some(100));
        assert_eq!(results.get("op2"), Some(200));
        assert_eq!(results.get("op3"), Some(300));
    }

    #[test]
    fn memory_usage_results_iter() {
        let mut results = MemoryUsageResults::new();

        results.add_explicit("op1".to_string(), 100);
        results.add_explicit("op2".to_string(), 200);

        let mut collected: Vec<_> = results.iter().collect();
        collected.sort_by_key(|(name, _)| name.as_str());

        assert_eq!(collected.len(), 2);
        assert_eq!(collected[0], (&"op1".to_string(), &100));
        assert_eq!(collected[1], (&"op2".to_string(), &200));
    }

    #[test]
    fn memory_usage_results_display() {
        let mut results = MemoryUsageResults::new();

        results.add_explicit("string_op".to_string(), 24);
        results.add_explicit("vector_op".to_string(), 800);

        let display_str = format!("{}", results);

        assert!(display_str.contains("Memory allocated:"));
        assert!(display_str.contains("string_op: 24 bytes per iteration"));
        assert!(display_str.contains("vector_op: 800 bytes per iteration"));
    }
}
