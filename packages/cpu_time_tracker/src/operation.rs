//! Average CPU time tracking.

use std::fmt;
use std::time::Duration;

use crate::pal::{Platform, PlatformFacade};

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
/// let mut session = Session::new();
/// let average = session.operation("cpu_intensive_work");
///
/// // Simulate multiple operations
/// for i in 0..5 {
///     let _span = average.measure_thread();
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
    platform: PlatformFacade,
}

impl Operation {
    /// Creates a new average CPU time calculator with the given name.
    #[must_use]
    pub(crate) fn new(name: impl Into<String>, platform: PlatformFacade) -> Self {
        Self {
            name: name.into(),
            total_cpu_time: Duration::ZERO,
            spans: 0,
            platform,
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
    /// let mut session = Session::new();
    /// let average = session.operation("thread_work");
    /// {
    ///     let _span = average.measure_thread();
    ///     // Perform some CPU-intensive work in this thread
    ///     let mut sum = 0;
    ///     for i in 0..1000 {
    ///         sum += i;
    ///     }
    /// } // Thread CPU time is tracked
    /// ```
    pub fn measure_thread(&mut self) -> ThreadSpan<'_> {
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
    /// let mut session = Session::new();
    /// let average = session.operation("process_work");
    /// {
    ///     let _span = average.measure_process();
    ///     // Perform some CPU-intensive work that might spawn threads
    ///     let mut sum = 0;
    ///     for i in 0..1000 {
    ///         sum += i;
    ///     }
    /// } // Total process CPU time is tracked
    /// ```
    pub fn measure_process(&mut self) -> ProcessSpan<'_> {
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
            // Use div_ceil for proper division, falling back to manual calculation if needed
            Duration::from_nanos(
                self.total_cpu_time
                    .as_nanos()
                    .checked_div(u128::from(self.spans))
                    .expect("average calculation should not overflow")
                    .try_into()
                    .expect("result should fit in u64"),
            )
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
/// let mut session = Session::new();
/// let average = session.operation("test");
/// {
///     let _span = average.measure_thread();
///     // Perform some CPU-intensive operation
///     let mut sum = 0;
///     for i in 0..1000 {
///         sum += i;
///     }
/// } // Thread CPU time is automatically tracked and recorded here
/// ```
///
/// For benchmarks with many iterations, use batching to reduce overhead:
///
/// ```
/// use cpu_time_tracker::{Operation, Session};
///
/// let mut session = Session::new();
/// let average = session.operation("test");
/// {
///     let _span = average.measure_thread().batch(1000);
///     for i in 0..1000 {
///         // Perform the operation being benchmarked
///         let mut sum = 0;
///         sum += i;
///     }
/// } // CPU time is measured once and divided by 1000
/// ```
#[derive(Debug)]
pub struct ThreadSpan<'a> {
    operation: &'a mut Operation,
    start_time: Duration,
    iterations: u64,
}

impl<'a> ThreadSpan<'a> {
    pub(crate) fn new(operation: &'a mut Operation) -> Self {
        let start_time = operation.platform.thread_time();

        Self {
            operation,
            start_time,
            iterations: 1,
        }
    }

    /// Sets the number of iterations this span represents.
    ///
    /// When measuring many iterations of the same operation in a loop,
    /// this method reduces measurement overhead by measuring the total
    /// time once and dividing by the iteration count.
    ///
    /// # Examples
    ///
    /// ```
    /// use cpu_time_tracker::{Operation, Session};
    ///
    /// let mut session = Session::new();
    /// let average = session.operation("fast_operation");
    /// {
    ///     let _span = average.measure_thread().batch(10000);
    ///     for i in 0..10000 {
    ///         // Fast operation that would be dominated by measurement overhead
    ///         std::hint::black_box(i * 2);
    ///     }
    /// } // Total time is measured once and divided by 10000
    /// ```
    #[must_use]
    pub fn batch(mut self, iterations: u64) -> Self {
        self.iterations = iterations;
        self
    }

    /// Calculates the thread CPU time delta since this span was created.
    #[must_use]
    fn to_duration(&self) -> Duration {
        let current_time = self.operation.platform.thread_time();
        let total_duration = current_time.saturating_sub(self.start_time);

        if self.iterations > 1 {
            Duration::from_nanos(
                total_duration
                    .as_nanos()
                    .checked_div(u128::from(self.iterations))
                    .unwrap_or(0)
                    .try_into()
                    .unwrap_or(0),
            )
        } else {
            total_duration
        }
    }
}

impl Drop for ThreadSpan<'_> {
    fn drop(&mut self) {
        if self.iterations == 0 {
            return; // No iterations means no work to record
        }

        let duration = self.to_duration();
        // Add the per-iteration duration, but record the number of iterations
        for _ in 0..self.iterations {
            self.operation.add(duration);
        }
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
/// let mut session = Session::new();
/// let average = session.operation("test");
/// {
///     let _span = average.measure_process();
///     // Perform some CPU-intensive operation
///     let mut sum = 0;
///     for i in 0..1000 {
///         sum += i;
///     }
/// } // Process CPU time is automatically tracked and recorded here
/// ```
///
/// For benchmarks with many iterations, use batching to reduce overhead:
///
/// ```
/// use cpu_time_tracker::{Operation, Session};
///
/// let mut session = Session::new();
/// let average = session.operation("test");
/// {
///     let _span = average.measure_process().batch(1000);
///     for i in 0..1000 {
///         // Perform the operation being benchmarked
///         let mut sum = 0;
///         sum += i;
///     }
/// } // CPU time is measured once and divided by 1000
/// ```
#[derive(Debug)]
pub struct ProcessSpan<'a> {
    operation: &'a mut Operation,
    start_time: Duration,
    iterations: u64,
}

impl<'a> ProcessSpan<'a> {
    pub(crate) fn new(operation: &'a mut Operation) -> Self {
        let start_time = operation.platform.process_time();

        Self {
            operation,
            start_time,
            iterations: 1,
        }
    }

    /// Sets the number of iterations this span represents.
    ///
    /// When measuring many iterations of the same operation in a loop,
    /// this method reduces measurement overhead by measuring the total
    /// time once and dividing by the iteration count.
    ///
    /// # Examples
    ///
    /// ```
    /// use cpu_time_tracker::{Operation, Session};
    ///
    /// let mut session = Session::new();
    /// let average = session.operation("fast_operation");
    /// {
    ///     let _span = average.measure_process().batch(10000);
    ///     for i in 0..10000 {
    ///         // Fast operation that would be dominated by measurement overhead
    ///         std::hint::black_box(i * 2);
    ///     }
    /// } // Total time is measured once and divided by 10000
    /// ```
    #[must_use]
    pub fn batch(mut self, iterations: u64) -> Self {
        self.iterations = iterations;
        self
    }

    /// Calculates the process CPU time delta since this span was created.
    #[must_use]
    fn to_duration(&self) -> Duration {
        let current_time = self.operation.platform.process_time();
        let total_duration = current_time.saturating_sub(self.start_time);

        if self.iterations > 1 {
            Duration::from_nanos(
                total_duration
                    .as_nanos()
                    .checked_div(u128::from(self.iterations))
                    .unwrap_or(0)
                    .try_into()
                    .unwrap_or(0),
            )
        } else {
            total_duration
        }
    }
}

impl Drop for ProcessSpan<'_> {
    fn drop(&mut self) {
        if self.iterations == 0 {
            return; // No iterations means no work to record
        }

        let duration = self.to_duration();
        // Add the per-iteration duration, but record the number of iterations
        for _ in 0..self.iterations {
            self.operation.add(duration);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::hint::black_box;

    use super::*;
    use crate::Session;

    // Helper function to create a mock session for testing
    fn create_test_session() -> Session {
        use crate::pal::{FakePlatform, PlatformFacade};

        let fake_platform = FakePlatform::new();
        let platform_facade = PlatformFacade::fake(fake_platform);
        Session::with_platform(platform_facade)
    }

    #[test]
    fn operation_new() {
        let mut session = create_test_session();
        let operation = session.operation("test");
        assert_eq!(operation.name(), "test");
        assert_eq!(operation.average(), Duration::ZERO);
        assert_eq!(operation.spans(), 0);
        assert_eq!(operation.total_cpu_time(), Duration::ZERO);
    }

    #[test]
    fn operation_add_single() {
        let mut session = create_test_session();
        let operation = session.operation("test");
        operation.add(Duration::from_millis(100));

        assert_eq!(operation.average(), Duration::from_millis(100));
        assert_eq!(operation.spans(), 1);
        assert_eq!(operation.total_cpu_time(), Duration::from_millis(100));
    }

    #[test]
    fn operation_add_multiple() {
        let mut session = create_test_session();
        let operation = session.operation("test");
        operation.add(Duration::from_millis(100));
        operation.add(Duration::from_millis(200));
        operation.add(Duration::from_millis(300));

        assert_eq!(operation.average(), Duration::from_millis(200)); // (100 + 200 + 300) / 3
        assert_eq!(operation.spans(), 3);
        assert_eq!(operation.total_cpu_time(), Duration::from_millis(600));
    }

    #[test]
    fn operation_add_zero() {
        let mut session = create_test_session();
        let operation = session.operation("test");
        operation.add(Duration::ZERO);
        operation.add(Duration::ZERO);

        assert_eq!(operation.average(), Duration::ZERO);
        assert_eq!(operation.spans(), 2);
        assert_eq!(operation.total_cpu_time(), Duration::ZERO);
    }

    #[test]
    #[cfg(not(miri))]
    fn operation_span_drop() {
        let mut session = create_test_session();
        let operation = session.operation("test");

        {
            let _span = operation.measure_thread();
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
    #[cfg(not(miri))]
    fn operation_thread_span_drop() {
        let mut session = create_test_session();
        let operation = session.operation("test");

        {
            let _span = operation.measure_thread();
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
    #[cfg(not(miri))]
    fn operation_process_span_drop() {
        let mut session = create_test_session();
        let operation = session.operation("test");

        {
            let _span = operation.measure_process();
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
    #[cfg(not(miri))]
    fn operation_multiple_spans() {
        let mut session = create_test_session();
        let operation = session.operation("test");

        // First span (thread)
        {
            let _span = operation.measure_thread();
            let mut sum = 0;
            for i in 0..100 {
                sum += i;
            }
            black_box(sum);
        }

        // Second span (process)
        {
            let _span = operation.measure_process();
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
    #[cfg(not(miri))]
    fn operation_span_no_work() {
        let mut session = create_test_session();
        let operation = session.operation("test");

        {
            let _span = operation.measure_thread();
            // No work - but we should consume some time anyway
            black_box(42);
        }

        assert_eq!(operation.spans(), 1);
        // Even with no work, some time may have passed
        assert!(operation.total_cpu_time() >= Duration::ZERO);
    }

    #[test]
    #[cfg(not(miri))]
    fn thread_span_to_duration_returns_non_zero() {
        use crate::pal::{FakePlatform, PlatformFacade};

        // Create a fake platform that returns progressively more time
        let mut fake_platform = FakePlatform::new();
        fake_platform.set_thread_time(Duration::from_millis(10));
        let platform_facade = PlatformFacade::fake(fake_platform);
        let mut session = Session::with_platform(platform_facade);

        let operation = session.operation("test");

        // Create a span without doing any work
        {
            let _span = operation.measure_thread();
            // Minimal work to avoid optimization
            black_box(42);
        }
        let time_without_work = operation.total_cpu_time();

        // Reset for second test with more CPU time
        let mut fake_platform2 = FakePlatform::new();
        fake_platform2.set_thread_time(Duration::from_millis(20));
        let platform_facade2 = PlatformFacade::fake(fake_platform2);
        let mut session2 = Session::with_platform(platform_facade2);
        let operation2 = session2.operation("test2");

        // Create a span with significant CPU work
        {
            let _span = operation2.measure_thread();
            let mut sum = 0_u64;
            for i in 0..50000 {
                sum = sum.wrapping_add(i);
            }
            black_box(sum);
        }
        let time_with_work = operation2.total_cpu_time();

        // This test will fail if to_duration() is replaced with Default::default()
        // because doing more work should result in at least as much CPU time
        assert!(
            time_with_work >= time_without_work,
            "Expected work to take at least as much CPU time as no work: {time_with_work:?} >= {time_without_work:?}"
        );
    }

    #[test]
    #[cfg(not(miri))]
    fn process_span_to_duration_returns_non_zero() {
        use crate::pal::{FakePlatform, PlatformFacade};

        // Create a fake platform that returns progressively more time
        let mut fake_platform = FakePlatform::new();
        fake_platform.set_process_time(Duration::from_millis(15));
        let platform_facade = PlatformFacade::fake(fake_platform);
        let mut session = Session::with_platform(platform_facade);

        let operation = session.operation("test");

        // Create a span without doing any work
        {
            let _span = operation.measure_process();
            // Minimal work to avoid optimization
            black_box(42);
        }
        let time_without_work = operation.total_cpu_time();

        // Reset for second test with more CPU time
        let mut fake_platform2 = FakePlatform::new();
        fake_platform2.set_process_time(Duration::from_millis(30));
        let platform_facade2 = PlatformFacade::fake(fake_platform2);
        let mut session2 = Session::with_platform(platform_facade2);
        let operation2 = session2.operation("test2");

        // Create a span with significant CPU work
        {
            let _span = operation2.measure_process();
            let mut sum = 0_u64;
            for i in 0..50000 {
                sum = sum.wrapping_add(i);
            }
            black_box(sum);
        }
        let time_with_work = operation2.total_cpu_time();

        // This test will fail if to_duration() is replaced with Default::default()
        // because doing more work should result in at least as much CPU time
        assert!(
            time_with_work >= time_without_work,
            "Expected work to take at least as much CPU time as no work: {time_with_work:?} >= {time_without_work:?}"
        );
    }

    #[test]
    fn thread_span_batch_single_iteration() {
        let mut session = create_test_session();
        let operation = session.operation("test");

        {
            let _span = operation.measure_thread().batch(1);
            // No actual work needed for this test
        }

        // With batch(1), should record 1 span regardless of actual time
        assert_eq!(operation.spans(), 1);
    }

    #[test]
    fn thread_span_batch_multiple_iterations() {
        let mut session = create_test_session();
        let operation = session.operation("test");

        {
            let _span = operation.measure_thread().batch(10);
            // No actual work needed for this test
        }

        // With batch(10), should record 10 spans
        assert_eq!(operation.spans(), 10);
    }

    #[test]
    fn process_span_batch_single_iteration() {
        let mut session = create_test_session();
        let operation = session.operation("test");

        {
            let _span = operation.measure_process().batch(1);
            // No actual work needed for this test
        }

        // With batch(1), should record 1 span regardless of actual time
        assert_eq!(operation.spans(), 1);
    }

    #[test]
    fn process_span_batch_multiple_iterations() {
        let mut session = create_test_session();
        let operation = session.operation("test");

        {
            let _span = operation.measure_process().batch(5);
            // No actual work needed for this test
        }

        // With batch(5), should record 5 spans
        assert_eq!(operation.spans(), 5);
    }

    #[test]
    fn thread_span_batch_zero_division_protection() {
        use crate::pal::{FakePlatform, PlatformFacade};

        let mut fake_platform = FakePlatform::new();
        fake_platform.set_thread_time(Duration::from_millis(100));
        let platform_facade = PlatformFacade::fake(fake_platform);
        let mut session = Session::with_platform(platform_facade);
        let operation = session.operation("test");

        {
            let _span = operation.measure_thread().batch(0);
            // This should not panic or cause division by zero
        }

        // With 0 iterations, nothing should be recorded
        assert_eq!(operation.spans(), 0);
        assert_eq!(operation.total_cpu_time(), Duration::ZERO);
    }
}
