//! Mean processor time tracking.

use std::fmt;
use std::time::Duration;

use crate::SpanBuilder;
use crate::pal::PlatformFacade;
/// Calculates mean processor time per operation across multiple iterations.
///
/// This utility is particularly useful for benchmarking scenarios where you want
/// to understand the mean processor time footprint of repeated operations.
///
/// # Examples
///
/// ```
/// use all_the_time::Session;
///
/// let mut session = Session::new();
/// let operation = session.operation("processor_intensive_work");
///
/// // Simulate multiple operations - note explicit iteration count
/// for i in 0..5 {
///     let _span = operation.iterations(1).measure_thread();
///     // Perform some processor-intensive work
///     let mut sum = 0;
///     for j in 0..i * 1000 {
///         sum += j;
///     }
/// }
///
/// let mean_duration = operation.mean();
/// println!("Mean processor time: {:?} per operation", mean_duration);
/// ```
#[derive(Debug)]
pub struct Operation {
    total_processor_time: Duration,
    spans: u64,
    platform: PlatformFacade,
}

impl Operation {
    /// Creates a new mean processor time calculator with the given name.
    #[must_use]
    pub(crate) fn new(platform: PlatformFacade) -> Self {
        Self {
            total_processor_time: Duration::ZERO,
            spans: 0,
            platform,
        }
    }

    /// Returns a reference to the platform facade for creating spans.
    #[must_use]
    pub(crate) fn platform(&self) -> &PlatformFacade {
        &self.platform
    }

    /// Adds a processor time duration to the mean calculation.
    ///
    /// This method is typically called by span types when they are dropped.
    pub(crate) fn add(&mut self, duration: Duration) {
        self.total_processor_time = self
            .total_processor_time
            .checked_add(duration)
            .expect("processor time duration overflow - this should not happen in practice");

        self.spans = self
            .spans
            .checked_add(1)
            .expect("span count overflow - this should not happen in practice");
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
    /// let mut session = Session::new();
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
    /// let mut session = Session::new();
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
    pub fn iterations(&mut self, iterations: u64) -> SpanBuilder<'_> {
        SpanBuilder::new(self, iterations)
    }

    /// Calculates the mean processor time per span.
    ///
    /// Returns zero duration if no spans have been recorded.
    #[must_use]
    pub fn mean(&self) -> Duration {
        if self.spans == 0 {
            Duration::ZERO
        } else {
            // Use div_ceil for proper division, falling back to manual calculation if needed
            Duration::from_nanos(
                self.total_processor_time
                    .as_nanos()
                    .checked_div(u128::from(self.spans))
                    .expect("mean calculation should not overflow")
                    .try_into()
                    .expect("result should fit in u64"),
            )
        }
    }

    /// Returns the total number of spans recorded.
    #[must_use]
    pub(crate) fn spans(&self) -> u64 {
        self.spans
    }

    /// Returns the total processor time across all spans.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn total_processor_time(&self) -> Duration {
        self.total_processor_time
    }
}

impl fmt::Display for Operation {
    #[cfg_attr(test, mutants::skip)] // No API contract.
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
        let mut session = create_test_session();
        let operation = session.operation("test");
        assert_eq!(operation.mean(), Duration::ZERO);
        assert_eq!(operation.spans(), 0);
        assert_eq!(operation.total_processor_time(), Duration::ZERO);
    }

    #[test]
    fn tracks_single_duration() {
        let mut session = create_test_session();
        let operation = session.operation("test");
        operation.add(Duration::from_millis(100));
        assert_eq!(operation.mean(), Duration::from_millis(100));
        assert_eq!(operation.spans(), 1);
        assert_eq!(operation.total_processor_time(), Duration::from_millis(100));
    }

    #[test]
    fn calculates_mean_of_multiple_durations() {
        let mut session = create_test_session();
        let operation = session.operation("test");
        operation.add(Duration::from_millis(100));
        operation.add(Duration::from_millis(200));
        operation.add(Duration::from_millis(300));
        assert_eq!(operation.mean(), Duration::from_millis(200)); // (100 + 200 + 300) / 3
        assert_eq!(operation.spans(), 3);
        assert_eq!(operation.total_processor_time(), Duration::from_millis(600));
    }

    #[test]
    fn handles_zero_durations() {
        let mut session = create_test_session();
        let operation = session.operation("test");
        operation.add(Duration::ZERO);
        operation.add(Duration::ZERO);
        assert_eq!(operation.mean(), Duration::ZERO);
        assert_eq!(operation.spans(), 2);
        assert_eq!(operation.total_processor_time(), Duration::ZERO);
    }

    #[test]
    fn integrates_with_span_api() {
        let mut session = create_test_session();
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

        assert_eq!(operation.spans(), 1);
        // We can't test the exact time, but it should be greater than zero
        assert!(operation.total_processor_time() >= Duration::ZERO);
    }

    #[test]
    fn handles_batch_iterations() {
        let mut session = create_test_session();
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

        assert_eq!(operation.spans(), 10);
        assert!(operation.total_processor_time() >= Duration::ZERO);
    }
}
