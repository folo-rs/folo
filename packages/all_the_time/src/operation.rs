//! Mean processor time tracking.

use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::constants::ERR_POISONED_LOCK;
use crate::SpanBuilder;
use crate::pal::PlatformFacade;
use crate::session::OperationMetrics;

/// Calculates mean processor time per operation across multiple iterations.
///
/// This utility is particularly useful for benchmarking scenarios where you want
/// to understand the mean processor time footprint of repeated operations.
///
/// Operations share data directly with the session - data is merged when spans are dropped.
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
///         let _span = operation.iterations(1).measure_thread();
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

    /// Adds a processor time duration to the mean calculation.
    ///
    /// This method is typically called by span types when they are dropped.
    /// Internally delegates to `add_iterations()` with a count of 1.
    ///
    /// # Panics
    ///
    /// Panics if the duration addition or iteration count would overflow.
    #[cfg(test)]
    pub(crate) fn add(&self, duration: Duration) {
        self.add_iterations(duration, 1);
    }

    /// Adds multiple iterations of the same duration to the mean calculation.
    ///
    /// This is a more efficient version of calling `add()` multiple times with the same duration.
    /// This method is used by span types when they measure multiple iterations.
    ///
    /// # Panics
    ///
    /// Panics if the duration multiplication or total time accumulation would overflow.
    #[cfg(test)]
    pub(crate) fn add_iterations(&self, duration: Duration, iterations: u64) {
        // Calculate total duration by multiplying duration by iterations
        let total_duration_nanos = duration.as_nanos()
            .checked_mul(u128::from(iterations))
            .expect("duration multiplied by iterations overflows u128 - this indicates an unrealistic scenario");

        let total_duration = Duration::from_nanos(
            total_duration_nanos
                .try_into()
                .expect("total duration exceeds maximum Duration value - this indicates an unrealistic scenario"),
        );

        // Add directly to operation data
        let mut data = self.metrics.lock().expect(ERR_POISONED_LOCK);
        data.total_processor_time = data.total_processor_time.checked_add(total_duration).expect(
            "processor time accumulation overflows Duration - this indicates an unrealistic scenario",
        );

        data.total_iterations = data.total_iterations.checked_add(iterations).expect(
            "total iterations count overflows u64 - this indicates an unrealistic scenario",
        );
    }

    /// Creates a span builder with the specified iteration count.
    ///
    /// This method requires an explicit iteration count, making the measurement
    /// overhead visible in the API. The iteration count cannot be zero.
    ///
    /// # Examples
    ///
    /// For single operations:
    /// ```
    /// use all_the_time::Session;
    ///
    /// let session = Session::new();
    /// let operation = session.operation("single_op");
    /// {
    ///     let _span = operation.iterations(1).measure_thread();
    ///     // Perform a single operation
    ///     let mut sum = 0;
    ///     for i in 0..1000 {
    ///         sum += i;
    ///     }
    /// }
    /// ```
    ///
    /// For batch operations (reduces measurement overhead):
    /// ```
    /// use all_the_time::Session;
    ///
    /// let session = Session::new();
    /// let operation = session.operation("batch_ops");
    /// {
    ///     let iterations = 10000;
    ///     let _span = operation.iterations(iterations).measure_thread();
    ///     for _ in 0..10000 {
    ///         // Fast operation that would be dominated by measurement overhead
    ///         std::hint::black_box(42 * 2);
    ///     }
    /// } // Total time is measured once and divided by 10000
    /// ```
    #[must_use]
    pub fn iterations(&self, iterations: u64) -> SpanBuilder<'_> {
        SpanBuilder::new(self, iterations)
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
        operation.add(Duration::from_millis(100));
        assert_eq!(operation.mean(), Duration::from_millis(100));
        assert_eq!(operation.total_iterations(), 1);
        assert_eq!(operation.total_processor_time(), Duration::from_millis(100));
    }

    #[test]
    fn calculates_mean_of_multiple_durations() {
        let session = create_test_session();
        let operation = session.operation("test");
        operation.add(Duration::from_millis(100));
        operation.add(Duration::from_millis(200));
        operation.add(Duration::from_millis(300));
        assert_eq!(operation.mean(), Duration::from_millis(200)); // (100 + 200 + 300) / 3
        assert_eq!(operation.total_iterations(), 3);
        assert_eq!(operation.total_processor_time(), Duration::from_millis(600));
    }

    #[test]
    fn handles_zero_durations() {
        let session = create_test_session();
        let operation = session.operation("test");
        operation.add(Duration::ZERO);
        operation.add(Duration::ZERO);
        assert_eq!(operation.mean(), Duration::ZERO);
        assert_eq!(operation.total_iterations(), 2);
        assert_eq!(operation.total_processor_time(), Duration::ZERO);
    }

    #[test]
    fn integrates_with_span_api() {
        let session = create_test_session();
        let operation = session.operation("test");
        {
            let _span = operation.iterations(1).measure_thread();
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
            let _span = operation.iterations(10).measure_thread();
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

    #[test]
    fn add_iterations_direct_call() {
        let session = create_test_session();
        let operation = session.operation("test");

        // Test direct call to add_iterations
        operation.add_iterations(Duration::from_millis(100), 5);

        assert_eq!(operation.total_iterations(), 5);
        assert_eq!(operation.total_processor_time(), Duration::from_millis(500));
        assert_eq!(operation.mean(), Duration::from_millis(100));
    }

    #[test]
    fn add_iterations_zero_iterations() {
        let session = create_test_session();
        let operation = session.operation("test");

        // Adding zero iterations should work and do nothing
        operation.add_iterations(Duration::from_millis(100), 0);

        assert_eq!(operation.total_iterations(), 0);
        assert_eq!(operation.total_processor_time(), Duration::ZERO);
        assert_eq!(operation.mean(), Duration::ZERO);
    }

    #[test]
    fn add_iterations_zero_duration() {
        let session = create_test_session();
        let operation = session.operation("test");

        // Adding zero duration should work
        operation.add_iterations(Duration::ZERO, 1000);

        assert_eq!(operation.total_iterations(), 1000);
        assert_eq!(operation.total_processor_time(), Duration::ZERO);
        assert_eq!(operation.mean(), Duration::ZERO);
    }

    // Static assertions for thread safety
    static_assertions::assert_impl_all!(Operation: Send);
    // Operation doesn't need to be Sync, only Send for thread mobility
}
