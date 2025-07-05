//! Average CPU time tracking.

use std::fmt;
use std::num::NonZero;
use std::time::Duration;

use crate::SpanBuilder;
use crate::pal::PlatformFacade;

/// Calculates average CPU time per operation across multiple iterations.
///
/// This utility is particularly useful for benchmarking scenarios where you want
/// to understand the average CPU time footprint of repeated operations.
///
/// # Examples
///
/// ```
/// use cpu_time_tracker::Session;
/// use std::num::NonZero;
///
/// let mut session = Session::new();
/// let operation = session.operation("cpu_intensive_work");
///
/// // Simulate multiple operations - note explicit iteration count
/// for i in 0..5 {
///     let _span = operation.iterations(NonZero::new(1).unwrap()).measure_thread();
///     // Perform some CPU-intensive work
///     let mut sum = 0;
///     for j in 0..i * 1000 {
///         sum += j;
///     }
/// }
///
/// let avg_duration = operation.average();
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

    /// Returns a reference to the platform facade for creating spans.
    #[must_use]
    pub(crate) fn platform(&self) -> &PlatformFacade {
        &self.platform
    }

    /// Adds a CPU time duration to the average calculation.
    ///
    /// This method is typically called by span types when they are dropped.
    pub(crate) fn add(&mut self, duration: Duration) {
        self.total_cpu_time = self
            .total_cpu_time
            .checked_add(duration)
            .expect("CPU time duration overflow - this should not happen in practice");
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
    /// use cpu_time_tracker::Session;
    /// use std::num::NonZero;
    ///
    /// let mut session = Session::new();
    /// let operation = session.operation("single_op");
    /// {
    ///     let _span = operation.iterations(NonZero::new(1).unwrap()).measure_thread();
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
    /// use cpu_time_tracker::Session;
    /// use std::num::NonZero;
    ///
    /// let mut session = Session::new();
    /// let operation = session.operation("batch_ops");
    /// {
    ///     let iterations = NonZero::new(10000).unwrap();
    ///     let _span = operation.iterations(iterations).measure_thread();
    ///     for _ in 0..10000 {
    ///         // Fast operation that would be dominated by measurement overhead
    ///         std::hint::black_box(42 * 2);
    ///     }
    /// } // Total time is measured once and divided by 10000
    /// ```
    #[must_use]
    pub fn iterations(&mut self, iterations: NonZero<u64>) -> SpanBuilder<'_> {
        SpanBuilder::new(self, iterations)
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

#[cfg(test)]
mod tests {
    use std::hint::black_box;
    use std::num::NonZero;

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
    fn operation_with_new_api() {
        let mut session = create_test_session();
        let operation = session.operation("test");

        {
            let _span = operation.iterations(NonZero::new(1).unwrap()).measure_thread();
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
    fn operation_batch_iterations() {
        let mut session = create_test_session();
        let operation = session.operation("test");

        {
            let _span = operation.iterations(NonZero::new(10).unwrap()).measure_thread();
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
        assert!(operation.total_cpu_time() >= Duration::ZERO);
    }
}
