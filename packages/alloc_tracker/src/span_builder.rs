//! Span builder for allocation tracking.

use crate::{Operation, ProcessSpan, ThreadSpan};

/// Builder for creating allocation tracking spans with explicit iteration counts.
///
/// This builder ensures that all allocation measurements require an explicit iteration
/// count, providing consistency with the `all_the_time` API and making the measurement
/// semantics explicit.
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
/// let operation = session.operation("test");
///
/// // Single iteration - explicit
/// {
///     let _span = operation.iterations(1).measure_thread();
///     // Perform work for a single iteration
///     let _data = vec![1, 2, 3];
/// }
///
/// // Multiple iterations - batch processing
/// {
///     let _span = operation.iterations(1000).measure_thread();
///     for _ in 0..1000 {
///         // Perform the same operation 1000 times
///         let _data = vec![42];
///     }
/// }
/// ```
#[derive(Debug)]
pub struct SpanBuilder<'a> {
    operation: &'a Operation,
    iterations: u64,
}

impl<'a> SpanBuilder<'a> {
    /// Creates a new span builder with the specified iteration count.
    ///
    /// # Panics
    ///
    /// Panics if `iterations` is zero.
    #[must_use]
    pub(crate) fn new(operation: &'a Operation, iterations: u64) -> Self {
        assert!(iterations != 0);

        Self {
            operation,
            iterations,
        }
    }

    /// Creates a span that tracks thread allocations from creation until it is dropped.
    ///
    /// This method tracks allocations made by the current thread only.
    /// Use this when you want to measure allocations for single-threaded operations
    /// or when you want to track per-thread allocation usage.
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
    ///     let _span = operation.iterations(1).measure_thread();
    ///     // Perform some allocation in this thread
    ///     let _data = vec![1, 2, 3, 4, 5];
    /// } // Thread allocations are tracked
    /// ```
    pub fn measure_thread(self) -> ThreadSpan {
        ThreadSpan::new(self.operation, self.iterations)
    }

    /// Creates a span that tracks process allocations from creation until it is dropped.
    ///
    /// This method tracks allocations made by the entire process (all threads).
    /// Use this when you want to measure total allocations including multi-threaded
    /// operations or when you want to track overall process allocation usage.
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
    ///     let _span = operation.iterations(1).measure_process();
    ///     // Perform some allocation that might span threads
    ///     let _data = vec![1, 2, 3, 4, 5];
    /// } // Total process allocations are tracked
    /// ```
    pub fn measure_process(self) -> ProcessSpan {
        ProcessSpan::new(self.operation, self.iterations)
    }
}
