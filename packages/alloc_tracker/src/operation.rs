//! Mean memory allocation tracking.

use std::fmt;

use crate::process_span::ProcessSpan;
use crate::thread_span::ThreadSpan;

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
///     let _span = mean_calc.measure_process();
///     let _data = vec![0; i + 1]; // Allocate different amounts
/// }
///
/// let mean_bytes = mean_calc.mean();
/// println!("Mean allocation: {} bytes per operation", mean_bytes);
/// ```
#[derive(Debug)]
pub struct Operation {
    total_bytes_allocated: u64,
    spans: u64,
}

impl Operation {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            total_bytes_allocated: 0,
            spans: 0,
        }
    }

    /// Adds a memory delta value to the mean calculation.
    ///
    /// This method is called by [`ProcessSpan`] or [`ThreadSpan`] when it is dropped.
    pub(crate) fn add(&mut self, delta: u64) {
        // Never going to overflow u64, so no point doing slower checked arithmetic here.
        self.total_bytes_allocated = self.total_bytes_allocated.wrapping_add(delta);
        self.spans = self.spans.wrapping_add(1);
    }

    /// Creates a span that is associated the the operation and will automatically
    /// track allocations from now until it is dropped.
    ///
    /// This method collects process-wide allocation data during the span.
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
    /// let mean_calc = session.operation("test");
    /// {
    ///     let _span = mean_calc.measure_process();
    ///     let _data = vec![1, 2, 3]; // This allocation will be tracked
    /// } // Span is dropped here, allocation is added to mean calculation
    /// ```
    pub fn measure_process(&mut self) -> ProcessSpan<'_> {
        ProcessSpan::new(self)
    }

    /// Creates a span that tracks allocations on this thread from now until it is dropped.
    ///
    /// This method tracks allocations made by the current thread only.
    /// Use this when you want to measure per-thread allocation patterns
    /// in multi-threaded scenarios.
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
    /// let mean_calc = session.operation("thread_work");
    /// {
    ///     let _span = mean_calc.measure_thread();
    ///     let _data = vec![1, 2, 3]; // This allocation will be tracked for this thread
    /// }
    /// ```
    pub fn measure_thread(&mut self) -> ThreadSpan<'_> {
        ThreadSpan::new(self)
    }

    /// Calculates the mean bytes allocated per span.
    ///
    /// Returns 0 if no spans have been recorded.
    #[expect(clippy::integer_division, reason = "we accept loss of precision")]
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "division by zero excluded via if-else"
    )]
    #[must_use]
    pub fn mean(&self) -> u64 {
        if self.spans == 0 {
            0
        } else {
            self.total_bytes_allocated / self.spans
        }
    }

    /// Returns the total number of spans recorded.
    #[must_use]
    pub(crate) fn spans(&self) -> u64 {
        self.spans
    }

    /// Returns the total bytes allocated across all spans.
    #[must_use]
    pub fn total_bytes_allocated(&self) -> u64 {
        self.total_bytes_allocated
    }
}

impl fmt::Display for Operation {
    #[cfg_attr(test, mutants::skip)] // No API contract.
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
        assert_eq!(operation.spans(), 0);
        assert_eq!(operation.total_bytes_allocated(), 0);
    }

    #[test]
    fn operation_add_single() {
        let mut operation = Operation::new();
        operation.add(100);

        assert_eq!(operation.mean(), 100);
        assert_eq!(operation.spans(), 1);
        assert_eq!(operation.total_bytes_allocated(), 100);
    }

    #[test]
    fn operation_add_multiple() {
        let mut operation = Operation::new();
        operation.add(100);
        operation.add(200);
        operation.add(300);

        assert_eq!(operation.mean(), 200); // (100 + 200 + 300) / 3
        assert_eq!(operation.spans(), 3);
        assert_eq!(operation.total_bytes_allocated(), 600);
    }

    #[test]
    fn operation_add_zero() {
        let mut operation = Operation::new();
        operation.add(0);
        operation.add(0);

        assert_eq!(operation.mean(), 0);
        assert_eq!(operation.spans(), 2);
        assert_eq!(operation.total_bytes_allocated(), 0);
    }

    #[test]
    fn operation_span_drop() {
        let mut operation = Operation::new();

        {
            let _span = operation.measure_process();
            // Simulate allocation
            TOTAL_BYTES_ALLOCATED.fetch_add(75, atomic::Ordering::Relaxed);
        }

        assert_eq!(operation.mean(), 75);
        assert_eq!(operation.spans(), 1);
        assert_eq!(operation.total_bytes_allocated(), 75);
    }

    #[test]
    fn operation_multiple_spans() {
        let mut operation = Operation::new();

        {
            let _span = operation.measure_process();
            TOTAL_BYTES_ALLOCATED.fetch_add(100, atomic::Ordering::Relaxed);
        }

        {
            let _span = operation.measure_process();
            TOTAL_BYTES_ALLOCATED.fetch_add(200, atomic::Ordering::Relaxed);
        }

        assert_eq!(operation.mean(), 150); // (100 + 200) / 2
        assert_eq!(operation.spans(), 2);
        assert_eq!(operation.total_bytes_allocated(), 300);
    }

    #[test]
    fn operation_process_span_no_allocation() {
        let mut operation = Operation::new();

        {
            let _span = operation.measure_process();
            // No allocation
        }

        assert_eq!(operation.mean(), 0);
        assert_eq!(operation.spans(), 1);
        assert_eq!(operation.total_bytes_allocated(), 0);
    }

    #[test]
    fn operation_thread_span_drop() {
        let mut operation = Operation::new();

        {
            let _span = operation.measure_thread();

            // Simulate thread-local allocation
            THREAD_BYTES_ALLOCATED.with(|counter| {
                counter.set(counter.get() + 50);
            });
        }

        assert_eq!(operation.mean(), 50);
        assert_eq!(operation.spans(), 1);
        assert_eq!(operation.total_bytes_allocated(), 50);
    }

    #[test]
    fn operation_mixed_spans() {
        let mut operation = Operation::new();

        {
            let _span = operation.measure_thread();
            THREAD_BYTES_ALLOCATED.with(|counter| {
                counter.set(counter.get() + 100);
            });
        }

        {
            let _span = operation.measure_process();
            TOTAL_BYTES_ALLOCATED.fetch_add(200, atomic::Ordering::Relaxed);
        }

        assert_eq!(operation.mean(), 150); // (100 + 200) / 2
        assert_eq!(operation.spans(), 2);
        assert_eq!(operation.total_bytes_allocated(), 300);
    }

    #[test]
    fn operation_thread_span_no_allocation() {
        let mut operation = Operation::new();

        {
            let _span = operation.measure_thread();
            // No allocation
        }

        assert_eq!(operation.mean(), 0);
        assert_eq!(operation.spans(), 1);
        assert_eq!(operation.total_bytes_allocated(), 0);
    }
}
