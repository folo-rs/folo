//! Span builder for processor time tracking.

use crate::{Operation, ProcessSpan, ThreadSpan};
/// Builder for creating processor time tracking spans with explicit iteration counts.
///
/// This builder ensures that all processor time measurements require an explicit iteration
/// count, making the measurement overhead visible in the API.
///
/// # Examples
///
/// ```
/// use all_the_time::Session;
///
/// let mut session = Session::new();
/// let operation = session.operation("test");
///
/// // Single iteration - explicit
/// {
///     let _span = operation.iterations(1).measure_thread();
///     // Perform work for a single iteration
/// }
///
/// // Multiple iterations - batch processing
/// {
///     let _span = operation.iterations(1000).measure_thread();
///     for _ in 0..1000 {
///         // Perform the same operation 1000 times
///     }
/// }
/// ```
#[derive(Debug)]
pub struct SpanBuilder<'a> {
    operation: &'a mut Operation,
    iterations: u64,
}

impl<'a> SpanBuilder<'a> {
    /// Creates a new span builder with the specified iteration count.
    ///
    /// # Panics
    ///
    /// Panics if `iterations` is zero.
    #[must_use]
    pub(crate) fn new(operation: &'a mut Operation, iterations: u64) -> Self {
        assert!(iterations != 0, "Iterations cannot be zero");

        Self {
            operation,
            iterations,
        }
    }

    /// Creates a span that tracks thread processor time from now until it is dropped.
    ///
    /// This method tracks processor time consumed by the current thread only.
    /// Use this when you want to measure processor time for single-threaded operations
    /// or when you want to track per-thread processor usage.
    ///
    /// # Examples
    ///
    /// ```
    /// use all_the_time::Session;
    ///
    /// let mut session = Session::new();
    /// let operation = session.operation("thread_work");
    /// {
    ///     let _span = operation.iterations(1).measure_thread();
    ///     // Perform some processor-intensive work in this thread
    ///     let mut sum = 0;
    ///     for i in 0..1000 {
    ///         sum += i;
    ///     }
    /// } // Thread processor time is tracked
    /// ```
    #[must_use]
    pub fn measure_thread(self) -> ThreadSpan<'a> {
        ThreadSpan::new(self.operation, self.iterations)
    }

    /// Creates a span that tracks process processor time from now until it is dropped.
    ///
    /// This method tracks processor time consumed by the entire process (all threads).
    /// Use this when you want to measure total processor time including multi-threaded
    /// operations or when you want to track overall process processor usage.
    ///
    /// # Examples
    ///
    /// ```
    /// use all_the_time::Session;
    ///
    /// let mut session = Session::new();
    /// let operation = session.operation("process_work");
    /// {
    ///     let _span = operation.iterations(1).measure_process();
    ///     // Perform some processor-intensive work that might spawn threads
    ///     let mut sum = 0;
    ///     for i in 0..1000 {
    ///         sum += i;
    ///     }
    /// } // Total process processor time is tracked
    /// ```
    #[must_use]
    pub fn measure_process(self) -> ProcessSpan<'a> {
        ProcessSpan::new(self.operation, self.iterations)
    }
}
