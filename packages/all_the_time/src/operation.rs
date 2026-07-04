//! Per-iteration processor time tracking.

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::pal::PlatformFacade;
use crate::statistics::nanos_to_duration;
use crate::{ERR_POISONED_LOCK, OperationMetrics, ProcessSpan, ThreadSpan};

/// Measures per-iteration processor time for a repeated operation.
///
/// This utility is particularly useful for benchmarking scenarios where you want
/// to understand the processor time footprint of repeated operations. The
/// headline figure is the warmup-robust per-iteration slope (see the crate-level
/// "Primary metric" documentation), with the pooled [`mean`](Self::mean) still
/// available.
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
/// # let session = session.no_stdout().no_file();
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
    /// # let session = session.no_stdout().no_file();
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
    /// # let session = session.no_stdout().no_file();
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
    /// # let session = session.no_stdout().no_file();
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
    /// # let session = session.no_stdout().no_file();
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
        self.metrics.lock().expect(ERR_POISONED_LOCK).mean()
    }

    /// Returns the total number of spans recorded.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn total_iterations(&self) -> u64 {
        self.metrics
            .lock()
            .expect(ERR_POISONED_LOCK)
            .total_iterations()
    }

    /// Returns the total processor time across all spans.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn total_processor_time(&self) -> Duration {
        self.metrics
            .lock()
            .expect(ERR_POISONED_LOCK)
            .total_processor_time()
    }
}

impl fmt::Display for Operation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.metrics.lock().expect(ERR_POISONED_LOCK).statistics() {
            Some(stats) => write!(
                f,
                "{:?} per iteration [{:?}, {:?}]",
                nanos_to_duration(stats.slope_nanos),
                nanos_to_duration(stats.interval_low_nanos),
                nanos_to_duration(stats.interval_high_nanos),
            ),
            None => write!(f, "no measurements"),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::hint::black_box;
    use std::panic::{RefUnwindSafe, UnwindSafe};

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
    static_assertions::assert_impl_all!(
        Operation: UnwindSafe, RefUnwindSafe
    );

    #[test]
    fn display_shows_robust_per_iteration_estimate() {
        let session = create_test_session();
        let operation = session.operation("test");

        // One span of 100ms per iteration across 2 iterations: the through-origin
        // slope recovers 100ms, and with a single span the interval collapses onto
        // it.
        {
            let mut metrics = operation.metrics.lock().expect(ERR_POISONED_LOCK);
            metrics.add_iterations(Duration::from_millis(100), 2);
        }

        let display = operation.to_string();
        assert!(
            display.contains("per iteration"),
            "Display should report the per-iteration estimate: got {display}"
        );
        // Duration debug format varies, but should contain '100' for the 100ms slope.
        assert!(
            display.contains("100"),
            "Display should show the 100ms per-iteration slope: got {display}"
        );
    }
}
