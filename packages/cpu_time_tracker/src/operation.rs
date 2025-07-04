//! Average CPU time tracking.

use std::fmt;
use std::time::Duration;

use cpu_time::{ProcessTime, ThreadTime};

/// Calculates average CPU time per operation across multiple iterations.
///
/// This utility is particularly useful for benchmarking scenarios where you want
/// to understand the average CPU time footprint of repeated operations.
///
/// # Examples
///
/// ```
/// use cpu_time_tracker::{Operation, Session};
///
/// let session = Session::new();
/// let mut average = Operation::new("cpu_intensive_work".to_string());
///
/// // Simulate multiple operations
/// for i in 0..5 {
///     let _span = average.thread_span();
///     // Perform some CPU-intensive work
///     let mut sum = 0;
///     for j in 0..i * 1000 {
///         sum += j;
///     }
/// }
///
/// let avg_duration = average.average();
/// println!("Average CPU time: {:?} per operation", avg_duration);
/// ```
#[derive(Debug)]
pub struct Operation {
    name: String,
    total_cpu_time: Duration,
    spans: u64,
}

impl Operation {
    /// Creates a new average CPU time calculator with the given name.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            total_cpu_time: Duration::ZERO,
            spans: 0,
        }
    }

    /// Returns the name of this operation.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Adds a CPU time duration to the average calculation.
    ///
    /// This method is typically called by [`Span`] when it is dropped.
    fn add(&mut self, duration: Duration) {
        self.total_cpu_time = self
            .total_cpu_time
            .checked_add(duration)
            .expect("CPU time duration overflow - this should not happen in practice");
        self.spans = self
            .spans
            .checked_add(1)
            .expect("span count overflow - this should not happen in practice");
    }

    /// Creates a span that tracks thread CPU time from now until it is dropped.
    ///
    /// This method tracks CPU time consumed by the current thread only.
    /// Use this when you want to measure CPU time for single-threaded operations
    /// or when you want to track per-thread CPU usage.
    ///
    /// # Examples
    ///
    /// ```
    /// use cpu_time_tracker::{Operation, Session};
    ///
    /// let session = Session::new();
    /// let mut average = Operation::new("thread_work".to_string());
    /// {
    ///     let _span = average.thread_span();
    ///     // Perform some CPU-intensive work in this thread
    ///     let mut sum = 0;
    ///     for i in 0..1000 {
    ///         sum += i;
    ///     }
    /// } // Thread CPU time is tracked
    /// ```
    pub fn thread_span(&mut self) -> ThreadSpan<'_> {
        ThreadSpan::new(self)
    }

    /// Creates a span that tracks process CPU time from now until it is dropped.
    ///
    /// This method tracks CPU time consumed by the entire process (all threads).
    /// Use this when you want to measure total CPU time including multi-threaded
    /// operations or when you want to track overall process CPU usage.
    ///
    /// # Examples
    ///
    /// ```
    /// use cpu_time_tracker::{Operation, Session};
    ///
    /// let session = Session::new();
    /// let mut average = Operation::new("process_work".to_string());
    /// {
    ///     let _span = average.process_span();
    ///     // Perform some CPU-intensive work that might spawn threads
    ///     let mut sum = 0;
    ///     for i in 0..1000 {
    ///         sum += i;
    ///     }
    /// } // Total process CPU time is tracked
    /// ```
    pub fn process_span(&mut self) -> ProcessSpan<'_> {
        ProcessSpan::new(self)
    }

    /// Calculates the average CPU time per span.
    ///
    /// Returns zero duration if no spans have been recorded.
    #[must_use]
    pub fn average(&self) -> Duration {
        if self.spans == 0 {
            Duration::ZERO
        } else {
            self.total_cpu_time
                .checked_div(self.spans.try_into().expect("spans count fits into u32"))
                .expect("average calculation should not overflow")
        }
    }

    /// Returns the total number of spans recorded.
    #[must_use]
    pub fn spans(&self) -> u64 {
        self.spans
    }

    /// Returns the total CPU time across all spans.
    #[must_use]
    pub fn total_cpu_time(&self) -> Duration {
        self.total_cpu_time
    }
}

impl fmt::Display for Operation {
    #[cfg_attr(test, mutants::skip)] // No API contract.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {:?} (mean)", self.name, self.average())
    }
}

/// A tracked span of code that tracks thread CPU time between creation and drop.
///
/// This span tracks CPU time consumed by the current thread only.
///
/// # Examples
///
/// ```
/// use cpu_time_tracker::{Operation, Session};
///
/// let session = Session::new();
/// let mut average = Operation::new("test".to_string());
/// {
///     let _span = average.thread_span();
///     // Perform some CPU-intensive operation
///     let mut sum = 0;
///     for i in 0..1000 {
///         sum += i;
///     }
/// } // Thread CPU time is automatically tracked and recorded here
/// ```
#[derive(Debug)]
pub struct ThreadSpan<'a> {
    operation: &'a mut Operation,
    start_time: ThreadTime,
}

impl<'a> ThreadSpan<'a> {
    pub(crate) fn new(operation: &'a mut Operation) -> Self {
        let start_time = ThreadTime::now();

        Self {
            operation,
            start_time,
        }
    }

    /// Calculates the thread CPU time delta since this span was created.
    #[must_use]
    fn to_duration(&self) -> Duration {
        self.start_time.elapsed()
    }
}

impl Drop for ThreadSpan<'_> {
    fn drop(&mut self) {
        let duration = self.to_duration();
        self.operation.add(duration);
    }
}

/// A tracked span of code that tracks process CPU time between creation and drop.
///
/// This span tracks CPU time consumed by the entire process (all threads).
///
/// # Examples
///
/// ```
/// use cpu_time_tracker::{Operation, Session};
///
/// let session = Session::new();
/// let mut average = Operation::new("test".to_string());
/// {
///     let _span = average.process_span();
///     // Perform some CPU-intensive operation
///     let mut sum = 0;
///     for i in 0..1000 {
///         sum += i;
///     }
/// } // Process CPU time is automatically tracked and recorded here
/// ```
#[derive(Debug)]
pub struct ProcessSpan<'a> {
    operation: &'a mut Operation,
    start_time: ProcessTime,
}

impl<'a> ProcessSpan<'a> {
    pub(crate) fn new(operation: &'a mut Operation) -> Self {
        let start_time = ProcessTime::now();

        Self {
            operation,
            start_time,
        }
    }

    /// Calculates the process CPU time delta since this span was created.
    #[must_use]
    fn to_duration(&self) -> Duration {
        self.start_time.elapsed()
    }
}

impl Drop for ProcessSpan<'_> {
    fn drop(&mut self) {
        let duration = self.to_duration();
        self.operation.add(duration);
    }
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;

    use super::*;
    use crate::Session;

    // Helper function to create a mock session for testing
    fn create_test_session() -> Session {
        Session::new()
    }

    #[test]
    fn operation_new() {
        let operation = Operation::new("test".to_string());
        assert_eq!(operation.name(), "test");
        assert_eq!(operation.average(), Duration::ZERO);
        assert_eq!(operation.spans(), 0);
        assert_eq!(operation.total_cpu_time(), Duration::ZERO);
    }

    #[test]
    fn operation_add_single() {
        let mut operation = Operation::new("test".to_string());
        operation.add(Duration::from_millis(100));

        assert_eq!(operation.average(), Duration::from_millis(100));
        assert_eq!(operation.spans(), 1);
        assert_eq!(operation.total_cpu_time(), Duration::from_millis(100));
    }

    #[test]
    fn operation_add_multiple() {
        let mut operation = Operation::new("test".to_string());
        operation.add(Duration::from_millis(100));
        operation.add(Duration::from_millis(200));
        operation.add(Duration::from_millis(300));

        assert_eq!(operation.average(), Duration::from_millis(200)); // (100 + 200 + 300) / 3
        assert_eq!(operation.spans(), 3);
        assert_eq!(operation.total_cpu_time(), Duration::from_millis(600));
    }

    #[test]
    fn operation_add_zero() {
        let mut operation = Operation::new("test".to_string());
        operation.add(Duration::ZERO);
        operation.add(Duration::ZERO);

        assert_eq!(operation.average(), Duration::ZERO);
        assert_eq!(operation.spans(), 2);
        assert_eq!(operation.total_cpu_time(), Duration::ZERO);
    }

    #[test]
    fn operation_span_drop() {
        let mut session = create_test_session();
        let operation = session.operation("test");

        {
            let _span = operation.thread_span();
            // Perform some CPU work
            let mut sum = 0;
            for i in 0..1000 {
                sum += i;
            }
            black_box(sum);
        } // Span drops here

        assert_eq!(operation.spans(), 1);
        // We can't test the exact time, but it should be greater than zero
        assert!(operation.total_cpu_time() >= Duration::ZERO);
    }

    #[test]
    fn operation_thread_span_drop() {
        let mut session = create_test_session();
        let operation = session.operation("test");

        {
            let _span = operation.thread_span();
            // Perform some CPU work
            let mut sum = 0;
            for i in 0..1000 {
                sum += i;
            }
            black_box(sum);
        } // ThreadSpan drops here

        assert_eq!(operation.spans(), 1);
        assert!(operation.total_cpu_time() >= Duration::ZERO);
    }

    #[test]
    fn operation_process_span_drop() {
        let mut session = create_test_session();
        let operation = session.operation("test");

        {
            let _span = operation.process_span();
            // Perform some CPU work
            let mut sum = 0;
            for i in 0..1000 {
                sum += i;
            }
            black_box(sum);
        } // ProcessSpan drops here

        assert_eq!(operation.spans(), 1);
        assert!(operation.total_cpu_time() >= Duration::ZERO);
    }

    #[test]
    fn operation_multiple_spans() {
        let mut session = create_test_session();
        let operation = session.operation("test");

        // First span (thread)
        {
            let _span = operation.thread_span();
            let mut sum = 0;
            for i in 0..100 {
                sum += i;
            }
            black_box(sum);
        }

        // Second span (process)
        {
            let _span = operation.process_span();
            let mut sum = 0;
            for i in 0..200 {
                sum += i;
            }
            black_box(sum);
        }

        assert_eq!(operation.spans(), 2);
        assert!(operation.total_cpu_time() >= Duration::ZERO);
    }

    #[test]
    fn operation_span_no_work() {
        let mut session = create_test_session();
        let operation = session.operation("test");

        {
            let _span = operation.thread_span();
            // No work - but we should consume some time anyway
            black_box(42);
        }

        assert_eq!(operation.spans(), 1);
        // Even with no work, some time may have passed
        assert!(operation.total_cpu_time() >= Duration::ZERO);
    }
}
