//! Results collection and display for memory usage measurements.

use std::collections::HashMap;
use std::fmt;

use crate::average::AverageMemoryDelta;

/// Results collector for memory usage measurements across multiple benchmarks or operations.
///
/// This type provides a convenient way to collect and display memory allocation measurements
/// from multiple operations or benchmark runs. It automatically extracts the name and
/// average from [`AverageMemoryDelta`] instances.
///
/// # Examples
///
/// ```
/// use std::alloc::System;
///
/// use allocation_tracker::{AllocationTrackingSession, AverageMemoryDelta, MemoryUsageResults, TrackingAllocator};
///
/// #[global_allocator]
/// static ALLOCATOR: TrackingAllocator<System> = TrackingAllocator::system();
///
/// let session = AllocationTrackingSession::new();
/// let mut results = MemoryUsageResults::new();
///
/// // Create measurements
/// let mut measurement1 = AverageMemoryDelta::new("operation_1".to_string());
/// {
///     let _contributor = measurement1.contribute(&session);
///     // Simulate some allocation
/// }
///
/// let mut measurement2 = AverageMemoryDelta::new("operation_2".to_string());
/// {
///     let _contributor = measurement2.contribute(&session);
///     // Simulate some allocation
/// }
///
/// // Add measurements to results
/// results.add(measurement1);
/// results.add(measurement2);
///
/// println!("{}", results);
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

    /// Adds a memory usage measurement from an [`AverageMemoryDelta`].
    ///
    /// The name and average from the measurement are automatically extracted.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::alloc::System;
    ///
    /// use allocation_tracker::{AllocationTrackingSession, AverageMemoryDelta, MemoryUsageResults, TrackingAllocator};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: TrackingAllocator<System> = TrackingAllocator::system();
    ///
    /// let session = AllocationTrackingSession::new();
    /// let mut results = MemoryUsageResults::new();
    /// let mut measurement = AverageMemoryDelta::new("test_op".to_string());
    ///
    /// // Perform measurements...
    /// results.add(measurement);
    /// ```
    #[expect(
        clippy::needless_pass_by_value,
        reason = "we need to consume the measurement to extract its data"
    )]
    pub fn add(&mut self, measurement: AverageMemoryDelta) {
        self.average_allocated_per_benchmark_iteration
            .insert(measurement.name().to_string(), measurement.average());
    }

    /// Adds a memory usage measurement with explicit name and average.
    ///
    /// This is a convenience method for cases where you have the data directly
    /// rather than from an [`AverageMemoryDelta`] instance.
    ///
    /// # Examples
    ///
    /// ```
    /// use allocation_tracker::MemoryUsageResults;
    ///
    /// let mut results = MemoryUsageResults::new();
    /// results.add_explicit("string_formatting".to_string(), 24);
    /// results.add_explicit("vector_allocation".to_string(), 800);
    /// ```
    pub fn add_explicit(&mut self, name: String, average: u64) {
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
    /// use std::alloc::System;
    ///
    /// use allocation_tracker::{AllocationTrackingSession, AverageMemoryDelta, MemoryUsageResults, TrackingAllocator};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: TrackingAllocator<System> = TrackingAllocator::system();
    ///
    /// let session = AllocationTrackingSession::new();
    /// let mut results = MemoryUsageResults::new();
    /// let measurement = AverageMemoryDelta::new("test_op".to_string());
    /// results.add(measurement);
    ///
    /// assert_eq!(results.get("test_op"), Some(0)); // No allocations recorded
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
    /// use std::alloc::System;
    ///
    /// use allocation_tracker::{AllocationTrackingSession, AverageMemoryDelta, MemoryUsageResults, TrackingAllocator};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: TrackingAllocator<System> = TrackingAllocator::system();
    ///
    /// let session = AllocationTrackingSession::new();
    /// let mut results = MemoryUsageResults::new();
    /// let measurement1 = AverageMemoryDelta::new("op1".to_string());
    /// let measurement2 = AverageMemoryDelta::new("op2".to_string());
    /// results.add(measurement1);
    /// results.add(measurement2);
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
    /// use std::alloc::System;
    ///
    /// use allocation_tracker::{AllocationTrackingSession, AverageMemoryDelta, MemoryUsageResults, TrackingAllocator};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: TrackingAllocator<System> = TrackingAllocator::system();
    ///
    /// let session = AllocationTrackingSession::new();
    /// let mut results = MemoryUsageResults::new();
    /// assert_eq!(results.len(), 0);
    ///
    /// let measurement = AverageMemoryDelta::new("test".to_string());
    /// results.add(measurement);
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
    /// use std::alloc::System;
    ///
    /// use allocation_tracker::{AllocationTrackingSession, AverageMemoryDelta, MemoryUsageResults, TrackingAllocator};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: TrackingAllocator<System> = TrackingAllocator::system();
    ///
    /// let session = AllocationTrackingSession::new();
    /// let mut results = MemoryUsageResults::new();
    /// assert!(results.is_empty());
    ///
    /// let measurement = AverageMemoryDelta::new("test".to_string());
    /// results.add(measurement);
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(collected.first(), Some(&(&"op1".to_string(), &100_u64)));
        assert_eq!(collected.get(1), Some(&(&"op2".to_string(), &200_u64)));
    }

    #[test]
    fn memory_usage_results_display() {
        let mut results = MemoryUsageResults::new();

        results.add_explicit("string_op".to_string(), 24);
        results.add_explicit("vector_op".to_string(), 800);

        let display_str = format!("{results}");

        assert!(display_str.contains("Memory allocated:"));
        assert!(display_str.contains("string_op: 24 bytes per iteration"));
        assert!(display_str.contains("vector_op: 800 bytes per iteration"));
    }
}
