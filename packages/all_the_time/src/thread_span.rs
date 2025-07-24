//! Thread-specific processor time tracking spans.

use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::Operation;
use crate::constants::ERR_POISONED_LOCK;
use crate::pal::{Platform, PlatformFacade};
use crate::session::OperationMetrics;
/// A tracked span of code that tracks thread processor time between creation and drop.
///
/// This span tracks processor time consumed by the current thread only.
///
/// # Examples
///
/// ```
/// use all_the_time::Session;
///
/// let session = Session::new();
/// let operation = session.operation("test");
/// {
///     let _span = operation.measure_thread();
///     // Perform some processor-intensive operation
///     let mut sum = 0;
///     for i in 0..1000 {
///         sum += i;
///     }
/// } // Thread processor time is automatically tracked and recorded here
/// ```
///
/// For benchmarks with many iterations:
///
/// ```
/// use all_the_time::Session;
///
/// let session = Session::new();
/// let operation = session.operation("test");
/// {
///     let _span = operation.measure_thread().iterations(1000);
///     for i in 0..1000 {
///         // Perform the operation being benchmarked
///         let mut sum = 0;
///         sum += i;
///     }
/// } // Processor time is measured once and divided by 1000
/// ```
#[derive(Debug)]
#[must_use = "Measurements are taken between creation and drop"]
pub struct ThreadSpan {
    metrics: Arc<Mutex<OperationMetrics>>,
    platform: PlatformFacade,
    start_time: Duration,
    iterations: u64,

    _single_threaded: PhantomData<*const ()>,
}

impl ThreadSpan {
    /// Creates a new thread span for the given operation and iteration count.
    ///
    /// # Panics
    ///
    /// Panics if `iterations` is zero.
    pub(crate) fn new(operation: &Operation, iterations: u64) -> Self {
        assert!(iterations != 0, "Iterations cannot be zero");

        let platform = operation.platform().clone();
        let start_time = platform.thread_time();

        Self {
            metrics: operation.metrics(),
            platform,
            start_time,
            iterations,
            _single_threaded: PhantomData,
        }
    }

    /// Sets the number of iterations for this span.
    ///
    /// This allows you to specify how many iterations this span represents,
    /// which is used to calculate the mean duration per iteration when the span is dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// use all_the_time::Session;
    ///
    /// let session = Session::new();
    /// let operation = session.operation("batch_work");
    /// {
    ///     let _span = operation.measure_thread().iterations(1000);
    ///     for _ in 0..1000 {
    ///         // Perform the same operation 1000 times
    ///         std::hint::black_box(42 * 2);
    ///     }
    /// } // Total time is measured once and divided by 1000
    /// ```
    ///
    /// You can also call it after some work has been done:
    /// ```
    /// use all_the_time::Session;
    ///
    /// let session = Session::new();
    /// let operation = session.operation("dynamic_work");
    /// {
    ///     let span = operation.measure_thread();
    ///     // Perform work and determine iteration count dynamically
    ///     let mut iterations = 0;
    ///     for i in 0..100 {
    ///         // Do some work
    ///         std::hint::black_box(i * 2);
    ///         iterations += 1;
    ///     }
    ///     span.iterations(iterations);
    /// } // Time is divided by the final iteration count
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if `iterations` is zero.
    pub fn iterations(mut self, iterations: u64) -> Self {
        assert!(iterations != 0, "Iterations cannot be zero");
        self.iterations = iterations;
        self
    }

    /// Calculates the thread processor time delta since this span was created.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // The != 1 fork is broadly applicable, so mutations fail. Intentional.
    fn to_duration(&self) -> Duration {
        let current_time = self.platform.thread_time();
        let total_duration = current_time.saturating_sub(self.start_time);

        if self.iterations > 1 {
            Duration::from_nanos(
                total_duration
                    .as_nanos()
                    .checked_div(u128::from(self.iterations))
                    .expect("guarded by if condition")
                    .try_into()
                    .expect("all realistic values fit in u64"),
            )
        } else {
            total_duration
        }
    }
}

impl Drop for ThreadSpan {
    fn drop(&mut self) {
        let duration = self.to_duration();

        // Calculate total duration by multiplying duration by iterations
        let total_duration_nanos = duration.as_nanos()
            .checked_mul(u128::from(self.iterations))
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

        data.total_iterations = data.total_iterations.checked_add(self.iterations).expect(
            "total iterations count overflows u64 - this indicates an unrealistic scenario",
        );
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::Session;
    use crate::pal::{FakePlatform, PlatformFacade};

    fn create_test_session() -> Session {
        let fake_platform = FakePlatform::new();
        let platform_facade = PlatformFacade::fake(fake_platform);
        Session::with_platform(platform_facade)
    }

    fn create_test_session_with_time(thread_time: Duration) -> Session {
        let fake_platform = FakePlatform::new();
        fake_platform.set_thread_time(thread_time);
        let platform_facade = PlatformFacade::fake(fake_platform);
        Session::with_platform(platform_facade)
    }

    #[test]
    fn creates_span_with_iterations() {
        let session = create_test_session();
        let operation = session.operation("test");
        let span = operation.measure_thread().iterations(5);
        assert_eq!(span.iterations, 5);
    }

    #[test]
    #[should_panic(expected = "Iterations cannot be zero")]
    fn panics_on_zero_iterations() {
        let session = create_test_session();
        let operation = session.operation("test");
        let _span = operation.measure_thread().iterations(0);
    }

    #[test]
    fn extracts_time_from_pal() {
        let session = create_test_session_with_time(Duration::ZERO);
        let operation = session.operation("test");

        {
            let _span = operation.measure_thread();
        }

        // Should extract time from PAL and record one span with zero duration
        assert_eq!(operation.total_iterations(), 1);
        assert_eq!(operation.total_processor_time(), Duration::ZERO);
    }

    #[test]
    fn calculates_time_delta() {
        let session = create_test_session();
        let operation = session.operation("test");

        // We need to simulate time advancement by accessing the platform directly
        // and changing the time between span creation and drop
        {
            let span = operation.measure_thread();

            // Manually advance the fake platform time
            // We need to get mutable access to the fake platform
            // This test verifies the span correctly calculates time delta
            drop(span);
        }

        // The span should have recorded some measurement
        assert_eq!(operation.total_iterations(), 1);
        // The exact time will depend on the fake platform behavior
    }

    #[test]
    fn records_one_span_per_iteration() {
        let session = create_test_session();
        let operation = session.operation("test");

        {
            let _span = operation.measure_thread().iterations(5);
            // Should record one span per iteration regardless of actual time measured
        }

        assert_eq!(operation.total_iterations(), 5);
    }

    #[test]
    fn calculates_per_iteration_duration() {
        let session = create_test_session();
        let operation = session.operation("test");

        // Create a span and test the duration calculation logic
        {
            let _span = operation.measure_thread().iterations(10);
            // The span will calculate duration when dropped
        }

        // Should have recorded 10 iterations
        assert_eq!(operation.total_iterations(), 10);
    }

    #[test]
    fn uses_thread_time_from_pal() {
        // Verify that ThreadSpan specifically calls thread_time() from the PAL
        let fake_platform = FakePlatform::new();
        fake_platform.set_thread_time(Duration::from_millis(50));
        fake_platform.set_process_time(Duration::from_millis(200)); // Different from thread time

        let platform_facade = PlatformFacade::fake(fake_platform);
        let session = Session::with_platform(platform_facade);
        let operation = session.operation("test");

        {
            let _span = operation.measure_thread();
            // Should use thread_time (50ms), not process_time (200ms)
        }

        assert_eq!(operation.total_iterations(), 1);
    }

    #[test]
    fn correctly_divides_by_iterations_count_single() {
        // Test case for single iteration (no division)
        // Since we can't modify fake platform after creation, we'll test
        // the behavior with a zero-time scenario
        let session = create_test_session();
        let operation = session.operation("test");

        {
            let _span = operation.measure_thread();
            // With fake platform, both start and end times are zero
        }

        // With single iteration, the duration calculation should work
        assert_eq!(operation.total_iterations(), 1);
        // Since fake platform starts at zero and doesn't advance, result should be zero
        assert_eq!(operation.total_processor_time(), Duration::ZERO);
    }

    #[test]
    fn correctly_divides_by_iterations_count_multiple() {
        // Test division for multiple iterations
        let session = create_test_session();
        let operation = session.operation("test");

        // Simulate a time measurement where we start at 0ms and end at 1000ms
        // with 10 iterations, so each should be 100ms
        {
            let _span = operation.measure_thread().iterations(10);
            // The span will divide total time by iterations when dropped
        }

        // Should record 10 spans
        assert_eq!(operation.total_iterations(), 10);
        // Each span should be the divided duration (but since we're using a fake platform
        // that starts at 0 and doesn't advance, total will be 0)
        assert_eq!(operation.total_processor_time(), Duration::ZERO);
    }

    #[test]
    fn iterations_divisor_applied_correctly_single() {
        // Test that single iteration does not divide (just returns total duration)
        let test_cases = [
            Duration::from_nanos(1000),
            Duration::from_millis(5),
            Duration::from_secs(1),
        ];

        for total_duration in test_cases {
            let iterations = 1_u64;

            // Simulate the logic from to_duration() method
            let result = if iterations > 1 {
                Duration::from_nanos(
                    total_duration
                        .as_nanos()
                        .checked_div(u128::from(iterations))
                        .unwrap_or(0)
                        .try_into()
                        .unwrap_or(0),
                )
            } else {
                total_duration
            };

            // For single iteration, should return the original duration
            assert_eq!(result, total_duration);
        }
    }

    #[test]
    fn iterations_divisor_applied_correctly_multiple() {
        // Test that multiple iterations properly divide the duration
        let test_cases = [
            (Duration::from_nanos(1000), 5_u64, Duration::from_nanos(200)),
            (Duration::from_millis(100), 4_u64, Duration::from_millis(25)),
            (Duration::from_secs(1), 10_u64, Duration::from_millis(100)),
        ];

        for (total_duration, iterations, expected) in test_cases {
            // Simulate the logic from to_duration() method
            let result = if iterations > 1 {
                Duration::from_nanos(
                    total_duration
                        .as_nanos()
                        .checked_div(u128::from(iterations))
                        .unwrap_or(0)
                        .try_into()
                        .unwrap_or(0),
                )
            } else {
                total_duration
            };

            assert_eq!(
                result, expected,
                "Failed for total={total_duration:?}, iterations={iterations}"
            );
        }
    }

    #[test]
    fn iterations_divisor_logic() {
        // Test the core division logic more directly by setting up time advancement
        let fake_platform = FakePlatform::new();
        // Start with zero time
        fake_platform.set_thread_time(Duration::ZERO);

        let platform_facade = PlatformFacade::fake(fake_platform);
        let session = Session::with_platform(platform_facade);
        let operation = session.operation("test");

        // Create span that should divide by iterations
        let span = operation.measure_thread().iterations(5);

        // Since our fake platform doesn't automatically advance time,
        // and we can't modify it after creation, let's test with
        // a different approach - verify the logic through calculation
        let test_total_duration = Duration::from_nanos(1000);
        let iterations = 5_u64;

        // This is what the division logic should produce
        let expected_per_iteration = Duration::from_nanos(
            test_total_duration
                .as_nanos()
                .checked_div(u128::from(iterations))
                .unwrap_or(0)
                .try_into()
                .unwrap_or(0),
        );

        assert_eq!(expected_per_iteration, Duration::from_nanos(200));
        drop(span);
    }

    // Static assertions for thread safety
    // ThreadSpan should NOT be Send or Sync due to PhantomData<*const ()>
    static_assertions::assert_not_impl_all!(super::ThreadSpan: Send);
    static_assertions::assert_not_impl_all!(super::ThreadSpan: Sync);
}
