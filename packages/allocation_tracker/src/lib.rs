//! Memory allocation tracking utilities for benchmarks and performance analysis.
//!
//! This crate provides utilities to track memory allocations during code execution,
//! enabling measurement of memory usage patterns in benchmarks and performance tests.
//!
//! The core functionality includes:
//! - [`MemoryTracker`] - An implementation of [`tracking_allocator::AllocationTracker`]
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
//! tracking-allocator = "0.4"
//! ```
//!
//! # Examples
//!
//! Basic usage for tracking memory allocations:
//!
//! ```
//! use allocation_tracker::{MemoryDeltaTracker, AverageMemoryDelta};
//!
//! // Track a single operation
//! let tracker = MemoryDeltaTracker::new();
//! let data = vec![1, 2, 3, 4, 5]; // This allocates memory
//! let delta = tracker.to_delta();
//! println!("Allocated {} bytes", delta);
//!
//! // Track average over multiple operations
//! let mut average = AverageMemoryDelta::new();
//! for _ in 0..10 {
//!     let _contributor = average.contribute();
//!     let _data = vec![1, 2, 3]; // This allocates memory
//! }
//! println!("Average allocation: {} bytes per operation", average.average());
//! ```
//!
//! ## Integration with tracking_allocator
//!
//! For real allocation tracking, you need to set up `tracking_allocator` as your global allocator.
//! This requires a binary crate (application), not a library crate:
//!
//! ```rust
//! use allocation_tracker::{MemoryTracker, MemoryDeltaTracker};
//! use tracking_allocator::{AllocationRegistry, Allocator};
//! use std::alloc::System;
//!
//! #[global_allocator]
//! static ALLOCATOR: Allocator<System> = Allocator::system();
//!
//! fn main() {
//!     // Set up tracking
//!     AllocationRegistry::set_global_tracker(MemoryTracker).unwrap();
//!     AllocationRegistry::enable_tracking();
//!     
//!     // Track a single operation
//!     let tracker = MemoryDeltaTracker::new();
//!     let data = vec![1, 2, 3, 4, 5]; // This allocates memory
//!     let bytes_allocated = tracker.to_delta();
//!     
//!     println!("Allocated {} bytes", bytes_allocated);
//!     
//!     // Clean up
//!     AllocationRegistry::disable_tracking();
//! }
//! ```
//!
//! ## Collecting Multiple Measurements
//!
//! Use [`MemoryUsageResults`] to collect and display measurements from different operations:
//!
//! ```
//! use allocation_tracker::{MemoryDeltaTracker, MemoryUsageResults};
//!
//! let mut results = MemoryUsageResults::new();
//!
//! // Simulate measurements (in real usage, you'd use a global allocator)
//! let tracker = MemoryDeltaTracker::new();
//! // let _vec = vec![1, 2, 3, 4, 5]; // This would allocate memory
//! results.add("vector_creation".to_string(), 40);
//!
//! let tracker = MemoryDeltaTracker::new();
//! // let _string = String::from("Hello, world!"); // This would allocate memory  
//! results.add("string_creation".to_string(), 24);
//!
//! // Print all results
//! println!("{}", results);
//!
//! // Access individual results
//! if let Some(bytes) = results.get("vector_creation") {
//!     println!("Vector creation used {} bytes", bytes);
//! }
//! ```
//!
//! ## Use in Criterion Benchmarks
//!
//! This crate integrates well with Criterion for memory-aware benchmarking:
//!
//! ```rust
//! use allocation_tracker::{MemoryTracker, AverageMemoryDelta};
//! use criterion::{criterion_group, criterion_main, Criterion};
//! use tracking_allocator::{AllocationRegistry, Allocator};
//! use std::alloc::System;
//!
//! #[global_allocator]
//! static ALLOCATOR: Allocator<System> = Allocator::system();
//!
//! fn bench_string_allocation(c: &mut Criterion) {
//!     AllocationRegistry::set_global_tracker(MemoryTracker).unwrap();
//!     AllocationRegistry::enable_tracking();
//!     
//!     let mut group = c.benchmark_group("memory_usage");
//!     
//!     group.bench_function("string_formatting", |b| {
//!         let mut average_memory_delta = AverageMemoryDelta::new();
//!
//!         b.iter(|| {
//!             let _contributor = average_memory_delta.contribute();
//!             let s = format!("Hello, {}!", "world");
//!             std::hint::black_box(s);
//!         });
//!
//!         println!("Average allocation: {} bytes", average_memory_delta.average());
//!     });
//!     
//!     group.finish();
//!     AllocationRegistry::disable_tracking();
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
//! This crate requires a global allocator to be set up using `tracking_allocator::Allocator`. 
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

#![allow(
    missing_docs,
    reason = "API documentation is provided at the crate level for now"
)]

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic;
use std::sync::atomic::AtomicU64;

use tracking_allocator::AllocationTracker;

// The tracking allocator works with static data all over the place, so this is how it be.
static TRACKER_BYTES_ALLOCATED: AtomicU64 = AtomicU64::new(0);

/// A memory allocation tracker that integrates with [`tracking_allocator`].
///
/// This tracker only counts allocations, not deallocations, as it is designed
/// to measure the memory footprint of operations rather than the net memory usage.
///
/// # Examples
///
/// ```
/// use allocation_tracker::MemoryTracker;
/// use tracking_allocator::{AllocationRegistry, AllocationTracker};
///
/// let tracker = MemoryTracker;
/// // The tracker would typically be registered with AllocationRegistry
/// // in an integration test or application that sets up a global allocator.
/// ```
#[derive(Debug)]
pub struct MemoryTracker;

impl AllocationTracker for MemoryTracker {
    fn allocated(
        &self,
        _addr: usize,
        object_size: usize,
        _wrapped_size: usize,
        _group_id: tracking_allocator::AllocationGroupId,
    ) {
        TRACKER_BYTES_ALLOCATED.fetch_add(object_size as u64, atomic::Ordering::Relaxed);
    }

    fn deallocated(
        &self,
        _addr: usize,
        _object_size: usize,
        _wrapped_size: usize,
        _source_group_id: tracking_allocator::AllocationGroupId,
        _current_group_id: tracking_allocator::AllocationGroupId,
    ) {
        // We intentionally do not track deallocations as we want to measure
        // total allocation footprint, not net memory usage.
    }
}

/// Tracks memory allocation changes over a specific time period.
///
/// This tracker captures the initial allocation count when created and can
/// calculate the delta (difference) in allocations when queried.
///
/// # Examples
///
/// ```
/// use allocation_tracker::MemoryDeltaTracker;
///
/// let tracker = MemoryDeltaTracker::new();
/// let data = vec![1, 2, 3, 4, 5]; // This allocates memory
/// let delta = tracker.to_delta();
/// // delta will be >= the size of the vector allocation
/// ```
#[derive(Debug)]
pub struct MemoryDeltaTracker {
    initial_bytes_allocated: u64,
}

impl MemoryDeltaTracker {
    /// Creates a new memory delta tracker, capturing the current allocation count.
    pub fn new() -> Self {
        let initial_bytes_allocated = TRACKER_BYTES_ALLOCATED.load(atomic::Ordering::Relaxed);

        Self {
            initial_bytes_allocated,
        }
    }

    /// Calculates the memory allocation delta since this tracker was created.
    ///
    /// # Panics
    ///
    /// Panics if the total bytes allocated has somehow decreased since the tracker
    /// was created, which should never happen in normal operation.
    pub fn to_delta(&self) -> u64 {
        let final_bytes_allocated = TRACKER_BYTES_ALLOCATED.load(atomic::Ordering::Relaxed);

        final_bytes_allocated
            .checked_sub(self.initial_bytes_allocated)
            .expect("total bytes allocated could not possibly decrease")
    }
}

impl Default for MemoryDeltaTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Calculates average memory allocation per operation across multiple iterations.
///
/// This utility is particularly useful for benchmarking scenarios where you want
/// to understand the average memory footprint of repeated operations.
///
/// # Examples
///
/// ```
/// use allocation_tracker::AverageMemoryDelta;
///
/// let mut average = AverageMemoryDelta::new();
///
/// // Simulate multiple operations
/// for i in 0..5 {
///     let _contributor = average.contribute();
///     let _data = vec![0; i + 1]; // Allocate different amounts
/// }
///
/// let avg_bytes = average.average();
/// println!("Average allocation: {} bytes per operation", avg_bytes);
/// ```
#[derive(Debug)]
pub struct AverageMemoryDelta {
    total_bytes_allocated: u64,
    iterations: u64,
}

impl AverageMemoryDelta {
    /// Creates a new average memory delta calculator.
    pub fn new() -> Self {
        Self {
            total_bytes_allocated: 0,
            iterations: 0,
        }
    }

    /// Adds a memory delta measurement to the average calculation.
    ///
    /// This method is typically called by [`AverageMemoryDeltaContributor`] when it is dropped.
    pub fn add(&mut self, delta: u64) {
        // Never going to overflow u64, so no point doing slower checked arithmetic here.
        self.total_bytes_allocated = self.total_bytes_allocated.wrapping_add(delta);
        self.iterations = self.iterations.wrapping_add(1);
    }

    /// Creates a contributor that will automatically measure and add a memory delta
    /// when it is dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// use allocation_tracker::AverageMemoryDelta;
    ///
    /// let mut average = AverageMemoryDelta::new();
    /// {
    ///     let _contributor = average.contribute();
    ///     let _data = vec![1, 2, 3]; // This allocation will be measured
    /// } // Contributor is dropped here, measurement is added to average
    /// ```
    pub fn contribute(&mut self) -> AverageMemoryDeltaContributor<'_> {
        AverageMemoryDeltaContributor::new(self)
    }

    /// Calculates the average bytes allocated per iteration.
    ///
    /// Returns 0 if no iterations have been recorded.
    #[expect(clippy::integer_division, reason = "we accept loss of precision")]
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "division by zero excluded via if-else"
    )]
    pub fn average(&self) -> u64 {
        if self.iterations == 0 {
            0
        } else {
            self.total_bytes_allocated / self.iterations
        }
    }

    /// Returns the total number of iterations recorded.
    pub fn iterations(&self) -> u64 {
        self.iterations
    }

    /// Returns the total bytes allocated across all iterations.
    pub fn total_bytes_allocated(&self) -> u64 {
        self.total_bytes_allocated
    }
}

impl Default for AverageMemoryDelta {
    fn default() -> Self {
        Self::new()
    }
}

/// A contributor to average memory delta calculation that automatically measures
/// memory allocation when dropped.
///
/// This type is created by [`AverageMemoryDelta::contribute()`] and should not be
/// constructed directly. When dropped, it automatically measures the memory delta
/// since its creation and adds it to the associated [`AverageMemoryDelta`].
///
/// # Examples
///
/// ```
/// use allocation_tracker::AverageMemoryDelta;
///
/// let mut average = AverageMemoryDelta::new();
/// {
///     let _contributor = average.contribute();
///     // Perform some operation that allocates memory
///     let _data = String::from("Hello, world!");
/// } // Memory delta is automatically measured and recorded here
/// ```
#[derive(Debug)]
pub struct AverageMemoryDeltaContributor<'a> {
    average_memory_delta: &'a mut AverageMemoryDelta,
    memory_delta_tracker: MemoryDeltaTracker,
}

impl<'a> AverageMemoryDeltaContributor<'a> {
    fn new(average_memory_delta: &'a mut AverageMemoryDelta) -> Self {
        let memory_delta_tracker = MemoryDeltaTracker::new();

        Self {
            average_memory_delta,
            memory_delta_tracker,
        }
    }
}

impl Drop for AverageMemoryDeltaContributor<'_> {
    fn drop(&mut self) {
        let delta = self.memory_delta_tracker.to_delta();
        self.average_memory_delta.add(delta);
    }
}

/// Results collector for memory usage measurements across multiple benchmarks or operations.
///
/// This type provides a convenient way to collect and display memory allocation measurements
/// from multiple operations or benchmark runs.
///
/// # Examples
///
/// ```
/// use allocation_tracker::MemoryUsageResults;
///
/// let mut results = MemoryUsageResults::new();
/// results.add("operation_1".to_string(), 100);
/// results.add("operation_2".to_string(), 250);
///
/// println!("{}", results);
/// // Output:
/// // Memory allocated:
/// // operation_1: 100 bytes per iteration
/// // operation_2: 250 bytes per iteration
/// ```
#[derive(Debug)]
pub struct MemoryUsageResults {
    average_allocated_per_benchmark_iteration: HashMap<String, u64>,
}

impl MemoryUsageResults {
    /// Creates a new empty results collector.
    #[must_use]
    pub fn new() -> Self {
        Self {
            average_allocated_per_benchmark_iteration: HashMap::new(),
        }
    }

    /// Adds a memory usage measurement for a named operation.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the operation or benchmark
    /// * `average` - The average bytes allocated per iteration for this operation
    ///
    /// # Examples
    ///
    /// ```
    /// use allocation_tracker::MemoryUsageResults;
    ///
    /// let mut results = MemoryUsageResults::new();
    /// results.add("string_formatting".to_string(), 24);
    /// results.add("vector_allocation".to_string(), 800);
    /// ```
    pub fn add(&mut self, name: String, average: u64) {
        self.average_allocated_per_benchmark_iteration
            .insert(name, average);
    }

    /// Returns the average bytes allocated for a specific operation.
    ///
    /// Returns `None` if no measurement exists for the given name.
    ///
    /// # Examples
    ///
    /// ```
    /// use allocation_tracker::MemoryUsageResults;
    ///
    /// let mut results = MemoryUsageResults::new();
    /// results.add("test_op".to_string(), 42);
    ///
    /// assert_eq!(results.get("test_op"), Some(42));
    /// assert_eq!(results.get("nonexistent"), None);
    /// ```
    #[must_use]
    pub fn get(&self, name: &str) -> Option<u64> {
        self.average_allocated_per_benchmark_iteration
            .get(name)
            .copied()
    }

    /// Returns an iterator over all operation names and their average allocations.
    ///
    /// # Examples
    ///
    /// ```
    /// use allocation_tracker::MemoryUsageResults;
    ///
    /// let mut results = MemoryUsageResults::new();
    /// results.add("op1".to_string(), 100);
    /// results.add("op2".to_string(), 200);
    ///
    /// for (name, bytes) in results.iter() {
    ///     println!("{}: {} bytes", name, bytes);
    /// }
    /// ```
    pub fn iter(&self) -> impl Iterator<Item = (&String, &u64)> {
        self.average_allocated_per_benchmark_iteration.iter()
    }

    /// Returns the number of operations recorded.
    ///
    /// # Examples
    ///
    /// ```
    /// use allocation_tracker::MemoryUsageResults;
    ///
    /// let mut results = MemoryUsageResults::new();
    /// assert_eq!(results.len(), 0);
    ///
    /// results.add("test".to_string(), 42);
    /// assert_eq!(results.len(), 1);
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        self.average_allocated_per_benchmark_iteration.len()
    }

    /// Returns `true` if no operations have been recorded.
    ///
    /// # Examples
    ///
    /// ```
    /// use allocation_tracker::MemoryUsageResults;
    ///
    /// let mut results = MemoryUsageResults::new();
    /// assert!(results.is_empty());
    ///
    /// results.add("test".to_string(), 42);
    /// assert!(!results.is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.average_allocated_per_benchmark_iteration.is_empty()
    }
}

impl Default for MemoryUsageResults {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for MemoryUsageResults {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Memory allocated:")?;

        for (name, average) in &self.average_allocated_per_benchmark_iteration {
            writeln!(f, "{name}: {average} bytes per iteration")?;
        }
        Ok(())
    }
}

/// Resets the internal allocation counter to zero.
///
/// This function is primarily intended for testing purposes to ensure a clean
/// state between test runs. It should not be used in production code as it can
/// interfere with concurrent allocation tracking.
///
/// # Examples
///
/// ```
/// use allocation_tracker::{reset_allocation_counter, MemoryDeltaTracker};
///
/// reset_allocation_counter();
/// let tracker = MemoryDeltaTracker::new();
/// let data = vec![1, 2, 3];
/// let delta = tracker.to_delta();
/// // delta now reflects only the allocation since reset
/// ```
pub fn reset_allocation_counter() {
    TRACKER_BYTES_ALLOCATED.store(0, atomic::Ordering::Relaxed);
}

/// Returns the current total bytes allocated since the counter was last reset.
///
/// This function provides direct access to the internal allocation counter
/// and is primarily intended for testing and diagnostic purposes.
///
/// # Examples
///
/// ```
/// use allocation_tracker::current_allocation_count;
///
/// let initial = current_allocation_count();
/// let data = vec![1, 2, 3, 4, 5];
/// let after = current_allocation_count();
/// assert!(after >= initial);
/// ```
pub fn current_allocation_count() -> u64 {
    TRACKER_BYTES_ALLOCATED.load(atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    //! Unit tests for allocation_tracker.
    //!
    //! These tests focus on the logic that doesn't require the MemoryTracker to be
    //! wired up as a global allocator. Integration tests for the full functionality
    //! are in the tests/ directory.

    use crate::{
        AverageMemoryDelta, MemoryDeltaTracker, MemoryUsageResults, reset_allocation_counter,
    };

    #[test]
    fn test_average_memory_delta_new() {
        let average = AverageMemoryDelta::new();
        assert_eq!(average.average(), 0);
        assert_eq!(average.iterations(), 0);
        assert_eq!(average.total_bytes_allocated(), 0);
    }

    #[test]
    fn test_average_memory_delta_default() {
        let average = AverageMemoryDelta::default();
        assert_eq!(average.average(), 0);
        assert_eq!(average.iterations(), 0);
        assert_eq!(average.total_bytes_allocated(), 0);
    }

    #[test]
    fn test_average_memory_delta_add_single() {
        let mut average = AverageMemoryDelta::new();
        average.add(100);

        assert_eq!(average.average(), 100);
        assert_eq!(average.iterations(), 1);
        assert_eq!(average.total_bytes_allocated(), 100);
    }

    #[test]
    fn test_average_memory_delta_add_multiple() {
        let mut average = AverageMemoryDelta::new();
        average.add(100);
        average.add(200);
        average.add(300);

        assert_eq!(average.average(), 200); // (100 + 200 + 300) / 3
        assert_eq!(average.iterations(), 3);
        assert_eq!(average.total_bytes_allocated(), 600);
    }

    #[test]
    fn test_average_memory_delta_add_zero() {
        let mut average = AverageMemoryDelta::new();
        average.add(0);
        average.add(0);

        assert_eq!(average.average(), 0);
        assert_eq!(average.iterations(), 2);
        assert_eq!(average.total_bytes_allocated(), 0);
    }

    #[test]
    fn test_memory_delta_tracker_new() {
        reset_allocation_counter();
        let tracker = MemoryDeltaTracker::new();
        assert_eq!(tracker.to_delta(), 0);
    }

    #[test]
    fn test_memory_delta_tracker_default() {
        reset_allocation_counter();
        let tracker = MemoryDeltaTracker::default();
        assert_eq!(tracker.to_delta(), 0);
    }

    #[test]
    fn test_memory_delta_tracker_with_simulated_allocation() {
        reset_allocation_counter();
        let tracker = MemoryDeltaTracker::new();

        // Simulate some allocation by manually adding to the counter
        crate::TRACKER_BYTES_ALLOCATED.fetch_add(150, std::sync::atomic::Ordering::Relaxed);

        assert_eq!(tracker.to_delta(), 150);
    }

    #[test]
    fn test_memory_delta_tracker_multiple_trackers() {
        reset_allocation_counter();
        let tracker1 = MemoryDeltaTracker::new();

        // Simulate allocation
        crate::TRACKER_BYTES_ALLOCATED.fetch_add(100, std::sync::atomic::Ordering::Relaxed);

        let tracker2 = MemoryDeltaTracker::new();

        // Simulate more allocation
        crate::TRACKER_BYTES_ALLOCATED.fetch_add(50, std::sync::atomic::Ordering::Relaxed);

        assert_eq!(tracker1.to_delta(), 150); // Total since tracker1 was created
        assert_eq!(tracker2.to_delta(), 50); // Only since tracker2 was created
    }

    #[test]
    fn test_average_memory_delta_contributor_drop() {
        reset_allocation_counter();
        let mut average = AverageMemoryDelta::new();

        {
            let _contributor = average.contribute();
            // Simulate allocation
            crate::TRACKER_BYTES_ALLOCATED.fetch_add(75, std::sync::atomic::Ordering::Relaxed);
        } // Contributor drops here

        assert_eq!(average.average(), 75);
        assert_eq!(average.iterations(), 1);
        assert_eq!(average.total_bytes_allocated(), 75);
    }

    #[test]
    fn test_average_memory_delta_multiple_contributors() {
        reset_allocation_counter();
        let mut average = AverageMemoryDelta::new();

        // First contributor
        {
            let _contributor = average.contribute();
            crate::TRACKER_BYTES_ALLOCATED.fetch_add(100, std::sync::atomic::Ordering::Relaxed);
        }

        // Second contributor
        {
            let _contributor = average.contribute();
            crate::TRACKER_BYTES_ALLOCATED.fetch_add(200, std::sync::atomic::Ordering::Relaxed);
        }

        assert_eq!(average.average(), 150); // (100 + 200) / 2
        assert_eq!(average.iterations(), 2);
        assert_eq!(average.total_bytes_allocated(), 300);
    }

    #[test]
    fn test_average_memory_delta_contributor_no_allocation() {
        reset_allocation_counter();
        let mut average = AverageMemoryDelta::new();

        {
            let _contributor = average.contribute();
            // No allocation
        }

        assert_eq!(average.average(), 0);
        assert_eq!(average.iterations(), 1);
        assert_eq!(average.total_bytes_allocated(), 0);
    }

    #[test]
    fn test_reset_and_current_allocation_count() {
        // Set some value
        crate::TRACKER_BYTES_ALLOCATED.store(500, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(crate::current_allocation_count(), 500);

        // Reset to zero
        reset_allocation_counter();
        assert_eq!(crate::current_allocation_count(), 0);
    }

    /// This test ensures our internal access to TRACKER_BYTES_ALLOCATED works correctly.
    #[test]
    fn test_internal_counter_access() {
        reset_allocation_counter();
        assert_eq!(crate::current_allocation_count(), 0);

        crate::TRACKER_BYTES_ALLOCATED.fetch_add(42, std::sync::atomic::Ordering::Relaxed);
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
    fn memory_usage_results_default() {
        let results = MemoryUsageResults::default();
        assert_eq!(results.len(), 0);
        assert!(results.is_empty());
    }

    #[test]
    fn memory_usage_results_add_and_get() {
        let mut results = MemoryUsageResults::new();

        results.add("test_op".to_string(), 42);
        assert_eq!(results.len(), 1);
        assert!(!results.is_empty());
        assert_eq!(results.get("test_op"), Some(42));
        assert_eq!(results.get("nonexistent"), None);
    }

    #[test]
    fn memory_usage_results_multiple_operations() {
        let mut results = MemoryUsageResults::new();

        results.add("op1".to_string(), 100);
        results.add("op2".to_string(), 200);
        results.add("op3".to_string(), 300);

        assert_eq!(results.len(), 3);
        assert_eq!(results.get("op1"), Some(100));
        assert_eq!(results.get("op2"), Some(200));
        assert_eq!(results.get("op3"), Some(300));
    }

    #[test]
    fn memory_usage_results_iter() {
        let mut results = MemoryUsageResults::new();

        results.add("op1".to_string(), 100);
        results.add("op2".to_string(), 200);

        let mut collected: Vec<_> = results.iter().collect();
        collected.sort_by_key(|(name, _)| name.as_str());

        assert_eq!(collected.len(), 2);
        assert_eq!(collected[0], (&"op1".to_string(), &100));
        assert_eq!(collected[1], (&"op2".to_string(), &200));
    }

    #[test]
    fn memory_usage_results_display() {
        let mut results = MemoryUsageResults::new();

        results.add("string_op".to_string(), 24);
        results.add("vector_op".to_string(), 800);

        let display_str = format!("{}", results);

        assert!(display_str.contains("Memory allocated:"));
        assert!(display_str.contains("string_op: 24 bytes per iteration"));
        assert!(display_str.contains("vector_op: 800 bytes per iteration"));
    }
}
