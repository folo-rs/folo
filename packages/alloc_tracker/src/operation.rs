//! Average memory allocation tracking.

use std::fmt;
use std::sync::atomic;

use crate::tracker::TOTAL_BYTES_ALLOCATED;

/// Calculates average memory allocation per operation across multiple iterations.
///
/// This utility is particularly useful for benchmarking scenarios where you want
/// to understand the average memory footprint of repeated operations.
///
/// # Examples
///
/// ```
/// use std::alloc::System;
///
/// use alloc_tracker::{Allocator, Operation, Session};
///
/// #[global_allocator]
/// static ALLOCATOR: Allocator<System> = Allocator::system();
///
/// let session = Session::new();
/// let mut average = Operation::new("string_allocations".to_string());
///
/// // Simulate multiple operations
/// for i in 0..5 {
///     let _span = average.span();
///     let _data = vec![0; i + 1]; // Allocate different amounts
/// }
///
/// let avg_bytes = average.average();
/// println!("Average allocation: {} bytes per operation", avg_bytes);
/// ```
#[derive(Debug)]
pub struct Operation {
    name: String,
    total_bytes_allocated: u64,
    iterations: u64,
}

impl Operation {
    /// Creates a new average memory delta calculator with the given name.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            total_bytes_allocated: 0,
            iterations: 0,
        }
    }

    /// Returns the name of this operation.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Adds a memory delta value to the average calculation.
    ///
    /// This method is typically called by [`OperationSpan`] when it is dropped.
    pub fn add(&mut self, delta: u64) {
        // Never going to overflow u64, so no point doing slower checked arithmetic here.
        self.total_bytes_allocated = self.total_bytes_allocated.wrapping_add(delta);
        self.iterations = self.iterations.wrapping_add(1);
    }

    /// Creates a span that is associated the the operation and will automatically
    /// track allocations from now until it is dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::alloc::System;
    ///
    /// use alloc_tracker::{Allocator, Operation, Session};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<System> = Allocator::system();
    ///
    /// let session = Session::new();
    /// let mut average = Operation::new("test".to_string());
    /// {
    ///     let _span = average.span();
    ///     let _data = vec![1, 2, 3]; // This allocation will be tracked
    /// } // Contributor is dropped here, allocation is added to average
    /// ```
    pub fn span(&mut self) -> OperationSpan<'_> {
        OperationSpan::new(self)
    }

    /// Calculates the average bytes allocated per iteration.
    ///
    /// Returns 0 if no iterations have been recorded.
    #[expect(clippy::integer_division, reason = "we accept loss of precision")]
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "division by zero excluded via if-else"
    )]
    #[must_use]
    pub fn average(&self) -> u64 {
        if self.iterations == 0 {
            0
        } else {
            self.total_bytes_allocated / self.iterations
        }
    }

    /// Returns the total number of iterations recorded.
    #[must_use]
    pub fn iterations(&self) -> u64 {
        self.iterations
    }

    /// Returns the total bytes allocated across all iterations.
    #[must_use]
    pub fn total_bytes_allocated(&self) -> u64 {
        self.total_bytes_allocated
    }
}

impl fmt::Display for Operation {
    #[cfg_attr(test, mutants::skip)] // No API contract.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} bytes (mean)", self.name, self.average())
    }
}

/// An allocation tracker span that is associated with an operation. It will track allocations
/// between creation and drop and contribute them to the operation's statistics.
///
/// # Examples
///
/// ```
/// use std::alloc::System;
///
/// use alloc_tracker::{Allocator, Operation, Session};
///
/// #[global_allocator]
/// static ALLOCATOR: Allocator<System> = Allocator::system();
///
/// let session = Session::new();
/// let mut average = Operation::new("test".to_string());
/// {
///     let _span = average.span();
///     // Perform some operation that allocates memory
///     let _data = String::from("Hello, world!");
/// } // Memory delta is automatically tracked and recorded here
/// ```
#[derive(Debug)]
pub struct OperationSpan<'a> {
    operation: &'a mut Operation,
    start_bytes: u64,
}

impl<'a> OperationSpan<'a> {
    pub(crate) fn new(operation: &'a mut Operation) -> Self {
        let start_bytes = TOTAL_BYTES_ALLOCATED.load(atomic::Ordering::Relaxed);

        Self {
            operation,
            start_bytes,
        }
    }

    /// Calculates the allocation delta since this span was created.
    #[must_use]
    pub fn to_delta(&self) -> u64 {
        let current_bytes = TOTAL_BYTES_ALLOCATED.load(atomic::Ordering::Relaxed);
        current_bytes
            .checked_sub(self.start_bytes)
            .expect("total bytes allocated could not possibly decrease")
    }
}

impl Drop for OperationSpan<'_> {
    fn drop(&mut self) {
        let delta = self.to_delta();
        self.operation.add(delta);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic;

    use super::*;
    use crate::Session;
    use crate::tracker::TOTAL_BYTES_ALLOCATED;

    // Helper function to create a mock session for testing
    // Note: This won't actually enable allocation tracking since we're not using
    // a global allocator in unit tests, but it allows us to test the API structure
    fn create_test_session() -> Session {
        // This function simplifies test setup by providing a session
        // We can now directly call new() since it's infallible
        Session::new()
    }

    #[test]
    fn average_memory_delta_new() {
        let average = Operation::new("test".to_string());
        assert_eq!(average.name(), "test");
        assert_eq!(average.average(), 0);
        assert_eq!(average.iterations(), 0);
        assert_eq!(average.total_bytes_allocated(), 0);
    }

    #[test]
    fn average_memory_delta_add_single() {
        let mut average = Operation::new("test".to_string());
        average.add(100);

        assert_eq!(average.average(), 100);
        assert_eq!(average.iterations(), 1);
        assert_eq!(average.total_bytes_allocated(), 100);
    }

    #[test]
    fn average_memory_delta_add_multiple() {
        let mut average = Operation::new("test".to_string());
        average.add(100);
        average.add(200);
        average.add(300);

        assert_eq!(average.average(), 200); // (100 + 200 + 300) / 3
        assert_eq!(average.iterations(), 3);
        assert_eq!(average.total_bytes_allocated(), 600);
    }

    #[test]
    fn average_memory_delta_add_zero() {
        let mut average = Operation::new("test".to_string());
        average.add(0);
        average.add(0);

        assert_eq!(average.average(), 0);
        assert_eq!(average.iterations(), 2);
        assert_eq!(average.total_bytes_allocated(), 0);
    }

    #[test]
    fn average_memory_delta_span_drop() {
        let mut session = create_test_session();
        let average = session.operation("test");

        {
            let _span = average.span();
            // Simulate allocation
            TOTAL_BYTES_ALLOCATED.fetch_add(75, atomic::Ordering::Relaxed);
        } // Contributor drops here

        assert_eq!(average.average(), 75);
        assert_eq!(average.iterations(), 1);
        assert_eq!(average.total_bytes_allocated(), 75);
    }

    #[test]
    fn average_memory_delta_multiple_spans() {
        let mut session = create_test_session();
        let average = session.operation("test");

        // First contributor
        {
            let _span = average.span();
            TOTAL_BYTES_ALLOCATED.fetch_add(100, atomic::Ordering::Relaxed);
        }

        // Second contributor
        {
            let _span = average.span();
            TOTAL_BYTES_ALLOCATED.fetch_add(200, atomic::Ordering::Relaxed);
        }

        assert_eq!(average.average(), 150); // (100 + 200) / 2
        assert_eq!(average.iterations(), 2);
        assert_eq!(average.total_bytes_allocated(), 300);
    }

    #[test]
    fn average_memory_delta_span_no_allocation() {
        let mut session = create_test_session();
        let average = session.operation("test");

        {
            let _span = average.span();
            // No allocation
        }

        assert_eq!(average.average(), 0);
        assert_eq!(average.iterations(), 1);
        assert_eq!(average.total_bytes_allocated(), 0);
    }
}
