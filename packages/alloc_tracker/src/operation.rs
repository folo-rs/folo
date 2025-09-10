//! Mean allocation tracking.

use std::fmt;
use std::sync::{Arc, Mutex};

use crate::{ERR_POISONED_LOCK, OperationMetrics, ProcessSpan, ThreadSpan};

/// A measurement handle for tracking mean memory allocation per operation across multiple iterations.
///
/// This utility is particularly useful for benchmarking scenarios where you want
/// to understand the mean memory footprint and allocation behavior of repeated operations.
/// It tracks both the number of bytes allocated and the count of allocations.
/// Operations share data with their parent session via reference counting, and data is
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
///     let _span = mean_calc.measure_process();
///     let _data = vec![0; i + 1]; // Allocate different amounts
/// }
///
/// let mean_bytes = mean_calc.mean();
/// println!("Mean allocation: {} bytes per operation", mean_bytes);
/// ```
#[derive(Debug)]
pub struct Operation {
    metrics: Arc<Mutex<OperationMetrics>>,
}

impl Operation {
    #[must_use]
    pub(crate) fn new(_name: String, operation_data: Arc<Mutex<OperationMetrics>>) -> Self {
        Self {
            metrics: operation_data,
        }
    }

    /// Returns a clone of the operation metrics for use by spans.
    #[must_use]
    pub(crate) fn metrics(&self) -> Arc<Mutex<OperationMetrics>> {
        Arc::clone(&self.metrics)
    }

    /// Creates a span that tracks thread allocations from creation until it is dropped.
    ///
    /// This method tracks allocations made by the current thread only.
    /// Use this when you want to measure allocations for single-threaded operations
    /// or when you want to track per-thread allocation usage.
    ///
    /// The span defaults to 1 iteration but can be changed using the `iterations()` method.
    ///
    /// # Examples
    ///
    /// ```
    /// use alloc_tracker::{Allocator, Session};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
    ///
    /// let session = Session::new();
    /// let operation = session.operation("thread_work");
    /// {
    ///     let _span = operation.measure_thread();
    ///     // Perform some allocation in this thread
    ///     let _data = vec![1, 2, 3, 4, 5];
    /// } // Thread allocations are tracked for 1 iteration
    /// ```
    ///
    /// For batch operations:
    /// ```
    /// use alloc_tracker::{Allocator, Session};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
    ///
    /// let session = Session::new();
    /// let operation = session.operation("batch_work");
    /// {
    ///     let _span = operation.measure_thread().iterations(1000);
    ///     for _ in 0..1000 {
    ///         // Perform the same operation 1000 times
    ///         let _data = vec![42];
    ///     }
    /// } // Total allocation is measured once and divided by 1000
    /// ```
    pub fn measure_thread(&self) -> ThreadSpan {
        ThreadSpan::new(self, 1)
    }

    /// Creates a span that tracks process allocations from creation until it is dropped.
    ///
    /// This method tracks allocations made by the entire process (all threads).
    /// Use this when you want to measure total allocations including multi-threaded
    /// operations or when you want to track overall process allocation usage.
    ///
    /// The span defaults to 1 iteration but can be changed using the `iterations()` method.
    ///
    /// # Examples
    ///
    /// ```
    /// use alloc_tracker::{Allocator, Session};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
    ///
    /// let session = Session::new();
    /// let operation = session.operation("process_work");
    /// {
    ///     let _span = operation.measure_process();
    ///     // Perform some allocation that might span threads
    ///     let _data = vec![1, 2, 3, 4, 5];
    /// } // Total process allocations are tracked for 1 iteration
    /// ```
    ///
    /// For batch operations:
    /// ```
    /// use alloc_tracker::{Allocator, Session};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
    ///
    /// let session = Session::new();
    /// let operation = session.operation("batch_work");
    /// {
    ///     let _span = operation.measure_process().iterations(1000);
    ///     for _ in 0..1000 {
    ///         // Perform the same operation 1000 times
    ///         let _data = vec![42];
    ///     }
    /// } // Total allocation is measured once and divided by 1000
    /// ```
    pub fn measure_process(&self) -> ProcessSpan {
        ProcessSpan::new(self, 1)
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
    use super::*;
    use crate::Session;
    use crate::allocator::register_fake_allocation;

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

        // Directly test the metrics
        {
            let mut metrics = operation.metrics.lock().expect(ERR_POISONED_LOCK);
            metrics.add_iterations(100, 1, 1);
        }

        assert_eq!(operation.mean(), 100);
        assert_eq!(operation.total_iterations(), 1);
        assert_eq!(operation.total_bytes_allocated(), 100);
    }

    #[test]
    fn operation_add_multiple() {
        let operation = create_test_operation();

        // Directly test the metrics
        {
            let mut metrics = operation.metrics.lock().expect(ERR_POISONED_LOCK);
            metrics.add_iterations(100, 1, 1); // 100 bytes, 1 allocation, 1 iteration
            metrics.add_iterations(200, 2, 1); // 200 bytes, 2 allocations, 1 iteration  
            metrics.add_iterations(300, 3, 1); // 300 bytes, 3 allocations, 1 iteration
        }

        assert_eq!(operation.mean(), 200); // (100 + 200 + 300) / (1 + 1 + 1) = 600 / 3 = 200
        assert_eq!(operation.total_iterations(), 3);
        assert_eq!(operation.total_bytes_allocated(), 600);
    }

    #[test]
    fn operation_add_zero() {
        let operation = create_test_operation();

        // Directly test the metrics
        {
            let mut metrics = operation.metrics.lock().expect(ERR_POISONED_LOCK);
            metrics.add_iterations(0, 0, 1);
            metrics.add_iterations(0, 0, 1);
        }

        assert_eq!(operation.mean(), 0);
        assert_eq!(operation.total_iterations(), 2);
        assert_eq!(operation.total_bytes_allocated(), 0);
    }

    #[test]
    fn operation_span_drop() {
        let operation = create_test_operation();

        {
            let _span = operation.measure_thread();
            // Simulate allocation
            register_fake_allocation(75, 1);
        }

        assert_eq!(operation.mean(), 75);
        assert_eq!(operation.total_iterations(), 1);
        assert_eq!(operation.total_bytes_allocated(), 75);
    }

    #[test]
    fn operation_multiple_spans() {
        let operation = create_test_operation();

        {
            let _span = operation.measure_thread();
            register_fake_allocation(100, 1);
        }

        {
            let _span = operation.measure_thread();
            register_fake_allocation(200, 1);
        }

        assert_eq!(operation.mean(), 150); // (100 + 200) / 2
        assert_eq!(operation.total_iterations(), 2);
        assert_eq!(operation.total_bytes_allocated(), 300);
    }

    #[test]
    fn operation_thread_span_drop() {
        let operation = create_test_operation();

        {
            let _span = operation.measure_thread();
            register_fake_allocation(50, 1);
        }

        assert_eq!(operation.mean(), 50);
        assert_eq!(operation.total_iterations(), 1);
        assert_eq!(operation.total_bytes_allocated(), 50);
    }

    #[test]
    fn operation_mixed_spans() {
        let operation = create_test_operation();

        {
            let _span = operation.measure_thread();
            register_fake_allocation(100, 1);
        }

        {
            let _span = operation.measure_thread();
            register_fake_allocation(200, 1);
        }

        assert_eq!(operation.mean(), 150); // (100 + 200) / 2
        assert_eq!(operation.total_iterations(), 2);
        assert_eq!(operation.total_bytes_allocated(), 300);
    }

    #[test]
    fn operation_thread_span_no_allocation() {
        let operation = create_test_operation();

        {
            let _span = operation.measure_thread();
            // No allocation
        }

        assert_eq!(operation.mean(), 0);
        assert_eq!(operation.total_iterations(), 1);
        assert_eq!(operation.total_bytes_allocated(), 0);
    }

    #[test]
    fn operation_batch_iterations() {
        let operation = create_test_operation();

        {
            let _span = operation.measure_thread().iterations(10);
            // Simulate a 1000 byte allocation that should be divided by 10 iterations
            register_fake_allocation(1000, 10);
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

            // Directly test the metrics
            {
                let mut metrics = operation.metrics.lock().expect(ERR_POISONED_LOCK);
                metrics.add_iterations(100, 2, 5);
            }
            // operation is dropped here, merging data into session
        }

        // Check that session contains the data
        let report = session.to_report();
        assert!(!report.is_empty());

        // Verify the session shows the data was merged
        let session_display = format!("{session}");
        println!("Actual session display: '{session_display}'");
        assert!(session_display.contains("| test      |        100 |          2 |")); // 500 bytes / 5 iterations = 100, 10 allocations / 5 iterations = 2
    }

    #[test]
    fn multiple_operations_concurrent() {
        let session = Session::new();

        let op1 = session.operation("test");
        let op2 = session.operation("test");

        // Directly manipulate the metrics
        {
            let mut metrics = op1.metrics.lock().expect(ERR_POISONED_LOCK);
            metrics.add_iterations(100, 1, 2); // 200 bytes, 2 allocations, 2 iterations
            metrics.add_iterations(200, 2, 3); // 600 bytes, 6 allocations, 3 iterations
        }

        // Both operations share the same data immediately since they have the same name
        // Total: 200 + 600 = 800 bytes, 2 + 3 = 5 iterations, mean = 800 / 5 = 160 bytes
        assert_eq!(op1.mean(), 160);
        assert_eq!(op2.mean(), 160);

        // Drop operations
        drop(op1);
        drop(op2);

        // Session should show merged results: 800 bytes, 5 iterations = 160 bytes mean, 8 allocations / 5 iterations = 1.6 ≈ 1 (integer division)
        let session_display = format!("{session}");
        assert!(session_display.contains("| test      |        160 |          1 |"));
    }

    static_assertions::assert_impl_all!(Operation: Send, Sync);
}
