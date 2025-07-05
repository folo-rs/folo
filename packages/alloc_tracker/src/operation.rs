//! Average memory allocation tracking.

use std::fmt;
use std::marker::PhantomData;
use std::sync::atomic;

use crate::allocator::{THREAD_BYTES_ALLOCATED, TOTAL_BYTES_ALLOCATED};

/// Calculates average memory allocation per operation across multiple iterations.
///
/// This utility is particularly useful for benchmarking scenarios where you want
/// to understand the average memory footprint of repeated operations.
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
/// let average = session.operation("string_allocations");
///
/// // Simulate multiple operations
/// for i in 0..5 {
///     let _span = average.measure_process();
///     let _data = vec![0; i + 1]; // Allocate different amounts
/// }
///
/// let avg_bytes = average.average();
/// println!("Average allocation: {} bytes per operation", avg_bytes);
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

    /// Adds a memory delta value to the average calculation.
    ///
    /// This method is called by [`ProcessSpan`] or [`ThreadSpan`] when it is dropped.
    fn add(&mut self, delta: u64) {
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
    /// let average = session.operation("test");
    /// {
    ///     let _span = average.measure_process();
    ///     let _data = vec![1, 2, 3]; // This allocation will be tracked
    /// } // Span is dropped here, allocation is added to average
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
    /// let average = session.operation("thread_work");
    /// {
    ///     let _span = average.measure_thread();
    ///     let _data = vec![1, 2, 3]; // This allocation will be tracked for this thread
    /// }
    /// ```
    pub fn measure_thread(&mut self) -> ThreadSpan<'_> {
        ThreadSpan::new(self)
    }

    /// Calculates the average bytes allocated per span.
    ///
    /// Returns 0 if no spans have been recorded.
    #[expect(clippy::integer_division, reason = "we accept loss of precision")]
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "division by zero excluded via if-else"
    )]
    #[must_use]
    pub fn average(&self) -> u64 {
        if self.spans == 0 {
            0
        } else {
            self.total_bytes_allocated / self.spans
        }
    }

    /// Returns the total number of spans recorded.
    #[must_use]
    pub fn spans(&self) -> u64 {
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
        write!(f, "{} bytes (mean)", self.average())
    }
}

/// A tracked span of code that tracks process-wide allocations between creation and drop.
///
/// This span tracks allocations made by the entire process (all threads).
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
/// let average = session.operation("test");
/// {
///     let _span = average.measure_process();
///     // Perform some operation that allocates memory
///     let _data = String::from("Hello, world!");
/// } // Memory delta is automatically tracked and recorded here
/// ```
#[derive(Debug)]
pub struct ProcessSpan<'a> {
    operation: &'a mut Operation,
    start_bytes: u64,
}

impl<'a> ProcessSpan<'a> {
    pub(crate) fn new(operation: &'a mut Operation) -> Self {
        let start_bytes = TOTAL_BYTES_ALLOCATED.load(atomic::Ordering::Relaxed);

        Self {
            operation,
            start_bytes,
        }
    }

    /// Calculates the allocation delta since this span was created.
    #[must_use]
    fn to_delta(&self) -> u64 {
        let current_bytes = TOTAL_BYTES_ALLOCATED.load(atomic::Ordering::Relaxed);
        current_bytes
            .checked_sub(self.start_bytes)
            .expect("total bytes allocated could not possibly decrease")
    }
}

impl Drop for ProcessSpan<'_> {
    fn drop(&mut self) {
        self.operation.add(self.to_delta());
    }
}

/// A tracked span of code that tracks allocations on this thread between creation and drop.
///
/// This span tracks allocations made by the current thread only.
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
/// let average = session.operation("test");
/// {
///     let _span = average.measure_thread();
///     // Perform some operation that allocates memory
///     let _data = String::from("Hello, world!");
/// } // Thread allocation is automatically tracked and recorded here
/// ```
#[derive(Debug)]
pub struct ThreadSpan<'a> {
    operation: &'a mut Operation,
    start_bytes: u64,

    _single_threaded: PhantomData<*const ()>,
}

impl<'a> ThreadSpan<'a> {
    pub(crate) fn new(operation: &'a mut Operation) -> Self {
        let start_bytes = THREAD_BYTES_ALLOCATED.with(std::cell::Cell::get);

        Self {
            operation,
            start_bytes,
            _single_threaded: PhantomData,
        }
    }

    /// Calculates the allocation delta since this span was created.
    #[must_use]
    fn to_delta(&self) -> u64 {
        let current_bytes = THREAD_BYTES_ALLOCATED.with(std::cell::Cell::get);
        current_bytes
            .checked_sub(self.start_bytes)
            .expect("thread bytes allocated could not possibly decrease")
    }
}

impl Drop for ThreadSpan<'_> {
    fn drop(&mut self) {
        self.operation.add(self.to_delta());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::allocator::{THREAD_BYTES_ALLOCATED, TOTAL_BYTES_ALLOCATED};

    #[test]
    fn operation_new() {
        let operation = Operation::new();
        assert_eq!(operation.average(), 0);
        assert_eq!(operation.spans(), 0);
        assert_eq!(operation.total_bytes_allocated(), 0);
    }

    #[test]
    fn operation_add_single() {
        let mut operation = Operation::new();
        operation.add(100);

        assert_eq!(operation.average(), 100);
        assert_eq!(operation.spans(), 1);
        assert_eq!(operation.total_bytes_allocated(), 100);
    }

    #[test]
    fn operation_add_multiple() {
        let mut operation = Operation::new();
        operation.add(100);
        operation.add(200);
        operation.add(300);

        assert_eq!(operation.average(), 200); // (100 + 200 + 300) / 3
        assert_eq!(operation.spans(), 3);
        assert_eq!(operation.total_bytes_allocated(), 600);
    }

    #[test]
    fn operation_add_zero() {
        let mut operation = Operation::new();
        operation.add(0);
        operation.add(0);

        assert_eq!(operation.average(), 0);
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

        assert_eq!(operation.average(), 75);
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

        assert_eq!(operation.average(), 150); // (100 + 200) / 2
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

        assert_eq!(operation.average(), 0);
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

        assert_eq!(operation.average(), 50);
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

        assert_eq!(operation.average(), 150); // (100 + 200) / 2
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

        assert_eq!(operation.average(), 0);
        assert_eq!(operation.spans(), 1);
        assert_eq!(operation.total_bytes_allocated(), 0);
    }
}
