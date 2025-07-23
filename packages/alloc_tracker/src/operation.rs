//! Mean allocation tracking.

use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use crate::SpanBuilder;
use crate::constants::ERR_POISONED_LOCK;
use crate::session::OperationMetrics;

/// Error returned when iteration count exceeds supported limits.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct AddIterationsError;

impl fmt::Display for AddIterationsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "iteration count overflow")
    }
}

impl std::error::Error for AddIterationsError {}

/// A measurement handle for tracking mean memory allocation per operation across multiple iterations.
///
/// This utility is particularly useful for benchmarking scenarios where you want
/// to understand the mean memory footprint of repeated operations. Operations
/// share data with their parent session via reference counting, and data is
/// merged when the operation is dropped.
///
/// Multiple operations with the same name can be created concurrently.
///
/// # Examples
///
/// ```
/// use alloc_tracker::{Allocator, Operation, Session};
///
/// #[global_allocator]
/// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
///
/// let session = Session::new();
/// let mean_calc = session.operation("string_allocations");
///
/// // Simulate multiple operations
/// for i in 0..5 {
///     let _span = mean_calc.iterations(1).measure_process();
///     let _data = vec![0; i + 1]; // Allocate different amounts
/// }
///
/// let mean_bytes = mean_calc.mean();
/// println!("Mean allocation: {} bytes per operation", mean_bytes);
/// ```
#[derive(Debug)]
pub struct Operation {
    metrics: Arc<Mutex<OperationMetrics>>,
    _not_sync: PhantomData<Cell<()>>,
}

impl Operation {
    #[must_use]
    pub(crate) fn new(_name: String, operation_data: Arc<Mutex<OperationMetrics>>) -> Self {
        Self {
            metrics: operation_data,
            _not_sync: PhantomData,
        }
    }

    /// Returns a clone of the operation metrics for use by spans.
    #[must_use]
    pub(crate) fn metrics(&self) -> Arc<Mutex<OperationMetrics>> {
        Arc::clone(&self.metrics)
    }

    /// Adds a memory delta value to the mean calculation.
    ///
    /// This method is called by [`ProcessSpan`] or [`ThreadSpan`] when it is dropped.
    /// Internally delegates to `add_iterations()` with a count of 1.
    #[cfg(test)]
    pub(crate) fn add(&self, delta: u64) {
        self.add_iterations(delta, 1);
    }

    /// Adds multiple iterations of the same allocation to the mean calculation.
    ///
    /// This is a more efficient version of calling `add()` multiple times with the same delta.
    /// This method is used by span types when they measure multiple iterations.
    #[cfg(test)]
    pub(crate) fn add_iterations(&self, delta: u64, iterations: u64) {
        let total_bytes = delta
            .checked_mul(iterations)
            .expect("bytes * iterations overflows u64 - this indicates an unrealistic scenario");

        let mut data = self.metrics.lock().expect(ERR_POISONED_LOCK);

        data.total_bytes_allocated = data
            .total_bytes_allocated
            .checked_add(total_bytes)
            .expect("total bytes allocated overflows u64 - this indicates an unrealistic scenario");

        data.total_iterations = data.total_iterations.checked_add(iterations).expect(
            "total iterations count overflows u64 - this indicates an unrealistic scenario",
        );
    }

    /// Creates a span builder with the specified iteration count.
    ///
    /// This method requires an explicit iteration count, making the measurement
    /// semantics consistent with the `all_the_time` API. The iteration count cannot be zero.
    ///
    /// # Examples
    ///
    /// For single operations:
    /// ```
    /// use alloc_tracker::{Allocator, Session};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
    ///
    /// let session = Session::new();
    /// let operation = session.operation("single_op");
    /// {
    ///     let _span = operation.iterations(1).measure_thread();
    ///     // Perform a single operation
    ///     let _data = vec![1, 2, 3];
    /// }
    /// ```
    ///
    /// For batch operations (consistent measurement):
    /// ```
    /// use alloc_tracker::{Allocator, Session};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
    ///
    /// let session = Session::new();
    /// let operation = session.operation("batch_ops");
    /// {
    ///     let iterations = 10000;
    ///     let _span = operation.iterations(iterations).measure_thread();
    ///     for _ in 0..10000 {
    ///         // Fast operation
    ///         let _data = vec![42];
    ///     }
    /// } // Total allocation is measured once and divided by 10000
    /// ```
    #[must_use]
    pub fn iterations(&self, iterations: u64) -> SpanBuilder<'_> {
        SpanBuilder::new(self, iterations)
    }

    /// Calculates the mean bytes allocated per iteration.
    ///
    /// Returns 0 if no iterations have been recorded.
    #[expect(clippy::integer_division, reason = "we accept loss of precision")]
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "division by zero excluded via if-else"
    )]
    #[must_use]
    pub fn mean(&self) -> u64 {
        let data = self.metrics.lock().expect(ERR_POISONED_LOCK);
        if data.total_iterations == 0 {
            0
        } else {
            data.total_bytes_allocated / data.total_iterations
        }
    }

    /// Returns the total number of iterations recorded.
    #[must_use]
    #[allow(dead_code, reason = "Used in tests")]
    #[cfg(test)]
    fn total_iterations(&self) -> u64 {
        let data = self.metrics.lock().expect(ERR_POISONED_LOCK);
        data.total_iterations
    }

    /// Returns the total bytes allocated across all iterations.
    #[must_use]
    pub fn total_bytes_allocated(&self) -> u64 {
        let data = self.metrics.lock().expect("ERR_POISONED_LOCK");
        data.total_bytes_allocated
    }
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} bytes (mean)", self.mean())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic;

    use super::*;
    use crate::Session;
    use crate::allocator::{THREAD_BYTES_ALLOCATED, TOTAL_BYTES_ALLOCATED};

    fn create_test_operation() -> Operation {
        let session = Session::new();
        session.operation("test")
    }

    #[test]
    fn operation_new() {
        let operation = create_test_operation();
        assert_eq!(operation.mean(), 0);
        assert_eq!(operation.total_iterations(), 0);
        assert_eq!(operation.total_bytes_allocated(), 0);
    }

    #[test]
    fn operation_add_single() {
        let operation = create_test_operation();
        operation.add(100);

        assert_eq!(operation.mean(), 100);
        assert_eq!(operation.total_iterations(), 1);
        assert_eq!(operation.total_bytes_allocated(), 100);
    }

    #[test]
    fn operation_add_multiple() {
        let operation = create_test_operation();
        operation.add(100);
        operation.add(200);
        operation.add(300);

        assert_eq!(operation.mean(), 200); // (100 + 200 + 300) / 3
        assert_eq!(operation.total_iterations(), 3);
        assert_eq!(operation.total_bytes_allocated(), 600);
    }

    #[test]
    fn operation_add_zero() {
        let operation = create_test_operation();
        operation.add(0);
        operation.add(0);

        assert_eq!(operation.mean(), 0);
        assert_eq!(operation.total_iterations(), 2);
        assert_eq!(operation.total_bytes_allocated(), 0);
    }

    #[test]
    fn operation_span_drop() {
        let operation = create_test_operation();

        {
            let _span = operation.iterations(1).measure_process();
            // Simulate allocation
            TOTAL_BYTES_ALLOCATED.fetch_add(75, atomic::Ordering::Relaxed);
        }

        assert_eq!(operation.mean(), 75);
        assert_eq!(operation.total_iterations(), 1);
        assert_eq!(operation.total_bytes_allocated(), 75);
    }

    #[test]
    fn operation_multiple_spans() {
        let operation = create_test_operation();

        {
            let _span = operation.iterations(1).measure_process();
            TOTAL_BYTES_ALLOCATED.fetch_add(100, atomic::Ordering::Relaxed);
        }

        {
            let _span = operation.iterations(1).measure_process();
            TOTAL_BYTES_ALLOCATED.fetch_add(200, atomic::Ordering::Relaxed);
        }

        assert_eq!(operation.mean(), 150); // (100 + 200) / 2
        assert_eq!(operation.total_iterations(), 2);
        assert_eq!(operation.total_bytes_allocated(), 300);
    }

    #[test]
    fn operation_process_span_no_allocation() {
        let operation = create_test_operation();

        {
            let _span = operation.iterations(1).measure_process();
            // No allocation
        }

        assert_eq!(operation.mean(), 0);
        assert_eq!(operation.total_iterations(), 1);
        assert_eq!(operation.total_bytes_allocated(), 0);
    }

    #[test]
    fn operation_thread_span_drop() {
        let operation = create_test_operation();

        {
            let _span = operation.iterations(1).measure_thread();

            // Simulate thread-local allocation
            THREAD_BYTES_ALLOCATED.with(|counter| {
                counter.set(counter.get() + 50);
            });
        }

        assert_eq!(operation.mean(), 50);
        assert_eq!(operation.total_iterations(), 1);
        assert_eq!(operation.total_bytes_allocated(), 50);
    }

    #[test]
    fn operation_mixed_spans() {
        let operation = create_test_operation();

        {
            let _span = operation.iterations(1).measure_thread();
            THREAD_BYTES_ALLOCATED.with(|counter| {
                counter.set(counter.get() + 100);
            });
        }

        {
            let _span = operation.iterations(1).measure_process();
            TOTAL_BYTES_ALLOCATED.fetch_add(200, atomic::Ordering::Relaxed);
        }

        assert_eq!(operation.mean(), 150); // (100 + 200) / 2
        assert_eq!(operation.total_iterations(), 2);
        assert_eq!(operation.total_bytes_allocated(), 300);
    }

    #[test]
    fn operation_thread_span_no_allocation() {
        let operation = create_test_operation();

        {
            let _span = operation.iterations(1).measure_thread();
            // No allocation
        }

        assert_eq!(operation.mean(), 0);
        assert_eq!(operation.total_iterations(), 1);
        assert_eq!(operation.total_bytes_allocated(), 0);
    }

    #[test]
    fn add_iterations_direct_call() {
        let operation = create_test_operation();

        // Test direct call to add_iterations
        operation.add_iterations(100, 5);

        assert_eq!(operation.total_iterations(), 5);
        assert_eq!(operation.total_bytes_allocated(), 500);
        assert_eq!(operation.mean(), 100);
    }

    #[test]
    fn add_iterations_zero_iterations() {
        let operation = create_test_operation();

        // Adding zero iterations should work and do nothing
        operation.add_iterations(100, 0);

        assert_eq!(operation.total_iterations(), 0);
        assert_eq!(operation.total_bytes_allocated(), 0);
        assert_eq!(operation.mean(), 0);
    }

    #[test]
    fn add_iterations_zero_allocation() {
        let operation = create_test_operation();

        // Adding zero allocation should work
        operation.add_iterations(0, 1000);

        assert_eq!(operation.total_iterations(), 1000);
        assert_eq!(operation.total_bytes_allocated(), 0);
        assert_eq!(operation.mean(), 0);
    }

    #[test]
    fn operation_batch_iterations() {
        let operation = create_test_operation();

        {
            let _span = operation.iterations(10).measure_process();
            // Simulate a 1000 byte allocation that should be divided by 10 iterations
            TOTAL_BYTES_ALLOCATED.fetch_add(1000, atomic::Ordering::Relaxed);
        }

        assert_eq!(operation.total_iterations(), 10);
        assert_eq!(operation.total_bytes_allocated(), 1000);
        assert_eq!(operation.mean(), 100); // 1000 / 10
    }

    #[test]
    fn operation_drop_merges_data() {
        let session = Session::new();

        // Create and use operation
        {
            let operation = session.operation("test");
            operation.add_iterations(100, 5);
            // operation is dropped here, merging data into session
        }

        // Check that session contains the data
        let report = session.to_report();
        assert!(!report.is_empty());

        // Verify the session shows the data was merged
        let session_display = format!("{session}");
        println!("Actual session display: '{session_display}'");
        assert!(session_display.contains("100 bytes (mean)")); // 500 bytes / 5 iterations = 100
    }

    #[test]
    fn multiple_operations_concurrent() {
        let session = Session::new();

        let op1 = session.operation("test");
        let op2 = session.operation("test");

        op1.add_iterations(100, 2); // 200 bytes, 2 iterations
        op2.add_iterations(200, 3); // 600 bytes, 3 iterations

        // Both operations share the same data immediately since they have the same name
        // Total: 800 bytes, 5 iterations = 160 bytes mean
        assert_eq!(op1.mean(), 160);
        assert_eq!(op2.mean(), 160);

        // Drop operations
        drop(op1);
        drop(op2);

        // Session should show merged results: 800 bytes, 5 iterations = 160 bytes mean
        let session_display = format!("{session}");
        assert!(session_display.contains("160 bytes (mean)"));
    }

    // Static assertions for thread safety
    static_assertions::assert_impl_all!(Operation: Send);
    static_assertions::assert_not_impl_any!(Operation: Sync);
    // Operation is Send but !Sync due to PhantomData<Cell<()>>
}
