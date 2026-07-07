//! Per-iteration processor time tracking.

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::pal::PlatformFacade;
use crate::statistics::format_slope_nanos;
use crate::{ERR_POISONED_LOCK, OperationMetrics, ProcessSpan, ThreadSpan};

/// Measures per-iteration processor time for a repeated operation.
///
/// This is useful in benchmarks where you want to understand the processor time
/// cost of a repeated operation. It reports the per-iteration processor time; a
/// plain [`mean`](Self::mean) is also available.
///
/// Operations share data directly with the session - data is merged when spans are dropped.
///
/// Multiple operations with the same name can be created concurrently.
///
/// # Examples
///
/// ```no_run
/// use std::hint::black_box;
/// use std::time::Instant;
///
/// use all_the_time::Session;
/// use criterion::Criterion;
///
/// fn bench(c: &mut Criterion) {
///     let session = Session::new();
///     let operation = session.operation("processor_intensive_work");
///     c.bench_function("processor_intensive_work", |b| {
///         b.iter_custom(|iters| {
///             let start = Instant::now();
///             let _span = operation.measure_thread().iterations(iters);
///
///             for _ in 0..iters {
///                 black_box(42_u64.wrapping_mul(2));
///             }
///
///             start.elapsed()
///         });
///     });
/// }
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

    /// Begins measuring processor time consumed by the current thread only.
    ///
    /// Use this for single-threaded operations or when you want to track
    /// per-thread processor usage. Call
    /// [`iterations(n)`](ThreadSpan::iterations) on the returned span to state how
    /// many iterations the measured work covers; the span records its measurement
    /// when it is dropped.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::hint::black_box;
    /// use std::time::Instant;
    ///
    /// use all_the_time::Session;
    /// use criterion::Criterion;
    ///
    /// fn bench(c: &mut Criterion) {
    ///     let session = Session::new();
    ///     let operation = session.operation("thread_work");
    ///     c.bench_function("thread_work", |b| {
    ///         b.iter_custom(|iters| {
    ///             let start = Instant::now();
    ///             let _span = operation.measure_thread().iterations(iters);
    ///
    ///             for _ in 0..iters {
    ///                 black_box(42_u64.wrapping_mul(2));
    ///             }
    ///
    ///             start.elapsed()
    ///         });
    ///     });
    /// }
    /// ```
    pub fn measure_thread(&self) -> ThreadSpan {
        ThreadSpan::new(self)
    }

    /// Begins measuring processor time consumed by the entire process (all
    /// threads).
    ///
    /// Use this to measure total processor time including multi-threaded work. Call
    /// [`iterations(n)`](ProcessSpan::iterations) on the returned span to state how
    /// many iterations the measured work covers; the span records its measurement
    /// when it is dropped.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use std::hint::black_box;
    /// use std::time::Instant;
    ///
    /// use all_the_time::Session;
    /// use criterion::Criterion;
    ///
    /// fn bench(c: &mut Criterion) {
    ///     let session = Session::new();
    ///     let operation = session.operation("process_work");
    ///     c.bench_function("process_work", |b| {
    ///         b.iter_custom(|iters| {
    ///             let start = Instant::now();
    ///             let _span = operation.measure_process().iterations(iters);
    ///
    ///             for _ in 0..iters {
    ///                 black_box(42_u64.wrapping_mul(2));
    ///             }
    ///
    ///             start.elapsed()
    ///         });
    ///     });
    /// }
    /// ```
    pub fn measure_process(&self) -> ProcessSpan {
        ProcessSpan::new(self)
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
        // The summary shows only the slope, so take the cheap slope-only path.
        match self.metrics.lock().expect(ERR_POISONED_LOCK).slope_nanos() {
            Some(slope_nanos) => {
                write!(f, "{} per iteration", format_slope_nanos(slope_nanos))
            }
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
            let _span = operation.measure_thread().iterations(1);
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
        // slope recovers 100ms (the Display shows only slopes, not intervals).
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

    #[test]
    fn display_reports_no_measurements_when_empty() {
        // An operation with no recorded spans has no statistics, so its Display takes
        // the `None` leg.
        let session = create_test_session();
        let operation = session.operation("test");
        assert_eq!(operation.to_string(), "no measurements");
    }
}
