//! Mean memory allocation tracking.

use std::fmt;

use crate::SpanBuilder;

/// Calculates mean memory allocation per operation across multiple iterations.
///
/// This utility is particularly useful for benchmarking scenarios where you want
/// to understand the mean memory footprint of repeated operations.
///
/// # Examples
///
/// ```
/// use alloc_tracker::{Allocator, Operation, Session};
///
/// #[global_allocator]
/// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
///
/// let mut session = Session::new();
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
    total_bytes_allocated: u64,
    total_iterations: u64,
}

impl Operation {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            total_bytes_allocated: 0,
            total_iterations: 0,
        }
    }

    /// Adds a memory delta value to the mean calculation.
    ///
    /// This method is called by [`ProcessSpan`] or [`ThreadSpan`] when it is dropped.
    /// Internally delegates to `add_iterations()` with a count of 1.
    #[cfg(test)]
    pub(crate) fn add(&mut self, delta: u64) {
        self.add_iterations(delta, 1);
    }

    /// Adds multiple iterations of the same allocation to the mean calculation.
    ///
    /// This is a more efficient version of calling `add()` multiple times with the same delta.
    /// This method is used by span types when they measure multiple iterations.
    pub(crate) fn add_iterations(&mut self, delta: u64, iterations: u64) {
        // Calculate total bytes by multiplying delta by iterations
        let total_bytes = delta
            .checked_mul(iterations)
            .expect("allocation delta multiplied by iterations overflows u64 - this indicates an unrealistic scenario");

        // Never going to overflow u64, so no point doing slower checked arithmetic here.
        self.total_bytes_allocated = self.total_bytes_allocated.wrapping_add(total_bytes);
        self.total_iterations = self.total_iterations.wrapping_add(iterations);
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
    /// let mut session = Session::new();
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
    /// let mut session = Session::new();
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
    pub fn iterations(&mut self, iterations: u64) -> SpanBuilder<'_> {
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
        if self.total_iterations == 0 {
            0
        } else {
            self.total_bytes_allocated / self.total_iterations
        }
    }

    /// Returns the total number of iterations recorded.
    #[must_use]
    pub(crate) fn total_iterations(&self) -> u64 {
        self.total_iterations
    }

    /// Returns the total bytes allocated across all iterations.
    #[must_use]
    pub fn total_bytes_allocated(&self) -> u64 {
        self.total_bytes_allocated
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
    use crate::allocator::{THREAD_BYTES_ALLOCATED, TOTAL_BYTES_ALLOCATED};

    #[test]
    fn operation_new() {
        let operation = Operation::new();
        assert_eq!(operation.mean(), 0);
        assert_eq!(operation.total_iterations(), 0);
        assert_eq!(operation.total_bytes_allocated(), 0);
    }

    #[test]
    fn operation_add_single() {
        let mut operation = Operation::new();
        operation.add(100);

        assert_eq!(operation.mean(), 100);
        assert_eq!(operation.total_iterations(), 1);
        assert_eq!(operation.total_bytes_allocated(), 100);
    }

    #[test]
    fn operation_add_multiple() {
        let mut operation = Operation::new();
        operation.add(100);
        operation.add(200);
        operation.add(300);

        assert_eq!(operation.mean(), 200); // (100 + 200 + 300) / 3
        assert_eq!(operation.total_iterations(), 3);
        assert_eq!(operation.total_bytes_allocated(), 600);
    }

    #[test]
    fn operation_add_zero() {
        let mut operation = Operation::new();
        operation.add(0);
        operation.add(0);

        assert_eq!(operation.mean(), 0);
        assert_eq!(operation.total_iterations(), 2);
        assert_eq!(operation.total_bytes_allocated(), 0);
    }

    #[test]
    fn operation_span_drop() {
        let mut operation = Operation::new();

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
        let mut operation = Operation::new();

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
        let mut operation = Operation::new();

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
        let mut operation = Operation::new();

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
        let mut operation = Operation::new();

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
        let mut operation = Operation::new();

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
        let mut operation = Operation::new();

        // Test direct call to add_iterations
        operation.add_iterations(100, 5);

        assert_eq!(operation.total_iterations(), 5);
        assert_eq!(operation.total_bytes_allocated(), 500);
        assert_eq!(operation.mean(), 100);
    }

    #[test]
    fn add_iterations_zero_iterations() {
        let mut operation = Operation::new();

        // Adding zero iterations should work and do nothing
        operation.add_iterations(100, 0);

        assert_eq!(operation.total_iterations(), 0);
        assert_eq!(operation.total_bytes_allocated(), 0);
        assert_eq!(operation.mean(), 0);
    }

    #[test]
    fn add_iterations_zero_allocation() {
        let mut operation = Operation::new();

        // Adding zero allocation should work
        operation.add_iterations(0, 1000);

        assert_eq!(operation.total_iterations(), 1000);
        assert_eq!(operation.total_bytes_allocated(), 0);
        assert_eq!(operation.mean(), 0);
    }

    #[test]
    fn operation_batch_iterations() {
        let mut operation = Operation::new();

        {
            let _span = operation.iterations(10).measure_process();
            // Simulate a 1000 byte allocation that should be divided by 10 iterations
            TOTAL_BYTES_ALLOCATED.fetch_add(1000, atomic::Ordering::Relaxed);
        }

        assert_eq!(operation.total_iterations(), 10);
        assert_eq!(operation.total_bytes_allocated(), 1000);
        assert_eq!(operation.mean(), 100); // 1000 / 10
    }
}
