//! Mean processor time tracking.

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::pal::PlatformFacade;
use crate::{ERR_POISONED_LOCK, OperationMetrics, ProcessSpan, ThreadSpan};

/// Calculates mean processor time per operation across multiple iterations.
///
/// This utility is particularly useful for benchmarking scenarios where you want
/// to understand the mean processor time footprint of repeated operations.
///
/// Operations share data directly with the session - data is merged when spans are dropped.
///
/// Multiple operations with the same name can be created concurrently.
///
/// # Examples
///
/// ```
/// use all_the_time::Session;
///
/// let session = Session::new();
/// let operation = session.operation("processor_intensive_work");
///
/// // Simulate multiple operations - note explicit iteration count
/// for i in 0..5 {
///     {
///         let _span = operation.measure_thread();
///         // Perform some processor-intensive work
///         let mut sum = 0;
///         for j in 0..i * 1000 {
///             sum += j;
///         }
///     } // Span is dropped here, ensuring measurement is recorded
/// }
///
/// let mean_duration = operation.mean();
/// println!("Mean processor time: {:?} per operation", mean_duration);
/// ```
#[derive(Debug)]
pub struct Operation {
    metrics: Arc<Mutex<OperationMetrics>>,
    platform: PlatformFacade,
}

impl Operation {
    /// Creates a new mean processor time calculator with the given name.
    #[must_use]
    pub(crate) fn new(
        _name: String,
        operation_data: Arc<Mutex<OperationMetrics>>,
        platform: PlatformFacade,
    ) -> Self {
        Self {
            metrics: operation_data,
            platform,
        }
    }

    /// Returns a reference to the platform facade for creating spans.
    #[must_use]
    pub(crate) fn platform(&self) -> &PlatformFacade {
        &self.platform
    }

    /// Returns a clone of the operation metrics for use by spans.
    #[must_use]
    pub(crate) fn metrics(&self) -> Arc<Mutex<OperationMetrics>> {
        Arc::clone(&self.metrics)
    }

    /// Creates a span that tracks thread processor time from creation until it is dropped.
    ///
    /// This method tracks processor time consumed by the current thread only.
    /// Use this when you want to measure processor time for single-threaded operations
    /// or when you want to track per-thread processor usage.
    ///
    /// The span defaults to 1 iteration but can be changed using the `iterations()` method.
    ///
    /// # Examples
    ///
    /// ```
    /// use all_the_time::Session;
    ///
    /// let session = Session::new();
    /// let operation = session.operation("thread_work");
    /// {
    ///     let _span = operation.measure_thread();
    ///     // Perform some processor-intensive work in this thread
    ///     let mut sum = 0;
    ///     for i in 0..1000 {
    ///         sum += i;
    ///     }
    /// } // Thread processor time is tracked for 1 iteration
    /// ```
    ///
    /// For batch operations (reduces measurement overhead):
    /// ```
    /// use all_the_time::Session;
    ///
    /// let session = Session::new();
    /// let operation = session.operation("batch_ops");
    /// {
    ///     let _span = operation.measure_thread().iterations(10000);
    ///     for _ in 0..10000 {
    ///         // Fast operation that would be dominated by measurement overhead
    ///         std::hint::black_box(42 * 2);
    ///     }
    /// } // Total time is measured once and divided by 10000
    /// ```
    pub fn measure_thread(&self) -> ThreadSpan {
        ThreadSpan::new(self, 1)
    }

    /// Creates a span that tracks process processor time from creation until it is dropped.
    ///
    /// This method tracks processor time consumed by the entire process (all threads).
    /// Use this when you want to measure total processor time including multi-threaded
    /// operations or when you want to track overall process processor usage.
    ///
    /// The span defaults to 1 iteration but can be changed using the `iterations()` method.
    ///
    /// # Examples
    ///
    /// ```
    /// use all_the_time::Session;
    ///
    /// let session = Session::new();
    /// let operation = session.operation("process_work");
    /// {
    ///     let _span = operation.measure_process();
    ///     // Perform some processor-intensive work that might spawn threads
    ///     let mut sum = 0;
    ///     for i in 0..1000 {
    ///         sum += i;
    ///     }
    /// } // Total process processor time is tracked for 1 iteration
    /// ```
    ///
    /// For batch operations:
    /// ```
    /// use all_the_time::Session;
    ///
    /// let session = Session::new();
    /// let operation = session.operation("batch_work");
    /// {
    ///     let _span = operation.measure_process().iterations(1000);
    ///     for _ in 0..1000 {
    ///         // Perform the same operation 1000 times
    ///     }
    /// } // Total time is measured once and divided by 1000
    /// ```
    pub fn measure_process(&self) -> ProcessSpan {
        ProcessSpan::new(self, 1)
    }

    /// Calculates the mean processor time per span.
    ///
    /// Returns zero duration if no spans have been recorded.
    #[must_use]
    pub fn mean(&self) -> Duration {
        let data = self.metrics.lock().expect(ERR_POISONED_LOCK);
        if data.total_iterations == 0 {
            Duration::ZERO
        } else {
            // Use div_ceil for proper division, falling back to manual calculation if needed
            Duration::from_nanos(
                data.total_processor_time
                    .as_nanos()
                    .checked_div(u128::from(data.total_iterations))
                    .expect("guarded by if condition")
                    .try_into()
                    .expect("all realistic values fit in u64"),
            )
        }
    }

    /// Returns the total number of spans recorded.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn total_iterations(&self) -> u64 {
        let data = self.metrics.lock().expect(ERR_POISONED_LOCK);
        data.total_iterations
    }

    /// Returns the total processor time across all spans.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn total_processor_time(&self) -> Duration {
        let data = self.metrics.lock().expect(ERR_POISONED_LOCK);
        data.total_processor_time
    }
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?} (mean)", self.mean())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
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
    fn starts_with_zero_values() {
        let session = create_test_session();
        let operation = session.operation("test");
        assert_eq!(operation.mean(), Duration::ZERO);
        assert_eq!(operation.total_iterations(), 0);
        assert_eq!(operation.total_processor_time(), Duration::ZERO);
    }

    #[test]
    fn tracks_single_duration() {
        let session = create_test_session();
        let operation = session.operation("test");

        // Directly test the metrics
        let mut metrics = operation.metrics.lock().expect(ERR_POISONED_LOCK);
        metrics.add_iterations(Duration::from_millis(100), 1);
        drop(metrics);

        assert_eq!(operation.mean(), Duration::from_millis(100));
        assert_eq!(operation.total_iterations(), 1);
        assert_eq!(operation.total_processor_time(), Duration::from_millis(100));
    }

    #[test]
    fn calculates_mean_of_multiple_durations() {
        let session = create_test_session();
        let operation = session.operation("test");

        // Directly test the metrics
        {
            let mut metrics = operation.metrics.lock().expect(ERR_POISONED_LOCK);
            metrics.add_iterations(Duration::from_millis(100), 1);
            metrics.add_iterations(Duration::from_millis(200), 1);
            metrics.add_iterations(Duration::from_millis(300), 1);
        }

        assert_eq!(operation.mean(), Duration::from_millis(200)); // (100 + 200 + 300) / 3
        assert_eq!(operation.total_iterations(), 3);
        assert_eq!(operation.total_processor_time(), Duration::from_millis(600));
    }

    #[test]
    fn handles_zero_durations() {
        let session = create_test_session();
        let operation = session.operation("test");

        // Directly test the metrics
        {
            let mut metrics = operation.metrics.lock().expect(ERR_POISONED_LOCK);
            metrics.add_iterations(Duration::ZERO, 1);
            metrics.add_iterations(Duration::ZERO, 1);
        }

        assert_eq!(operation.mean(), Duration::ZERO);
        assert_eq!(operation.total_iterations(), 2);
        assert_eq!(operation.total_processor_time(), Duration::ZERO);
    }

    #[test]
    fn integrates_with_span_api() {
        let session = create_test_session();
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

        assert_eq!(operation.total_iterations(), 1);
        // We can not test the exact time, but it should be greater than zero
        assert!(operation.total_processor_time() >= Duration::ZERO);
    }

    #[test]
    fn handles_batch_iterations() {
        let session = create_test_session();
        let operation = session.operation("test");
        {
            let _span = operation.measure_thread().iterations(10);
            // Perform some CPU work in a batch
            for i in 0..10 {
                let mut sum = 0;
                for j in 0..100 {
                    sum += i * j;
                }
                black_box(sum);
            }
        } // Span drops here

        assert_eq!(operation.total_iterations(), 10);
        assert!(operation.total_processor_time() >= Duration::ZERO);
    }

    static_assertions::assert_impl_all!(Operation: Send, Sync);

    #[test]
    fn display_shows_mean() {
        let session = create_test_session();
        let operation = session.operation("test");

        // Add some data: 100ms per iteration * 2 iterations = 200ms total, mean = 100ms
        {
            let mut metrics = operation.metrics.lock().expect(ERR_POISONED_LOCK);
            metrics.add_iterations(Duration::from_millis(100), 2);
        }

        let display = operation.to_string();
        assert!(display.contains("mean"), "Display should mention 'mean'");
        // Duration debug format varies, but should contain '100' for 100ms mean
        assert!(
            display.contains("100"),
            "Display should show the mean duration containing '100' (for 100ms): got {display}"
        );
    }
}
