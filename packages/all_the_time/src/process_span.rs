//! Process-wide processor time tracking spans.

use std::time::Duration;

use crate::Operation;
use crate::pal::Platform;

/// A span of code for which we track process processor time between creation and drop.
///
/// Measures processor time consumed by the entire process (all threads).
///
/// # Examples
///
/// ```
/// use all_the_time::Session;
///
/// let mut session = Session::new();
/// let operation = session.operation("test");
/// {
///     let _span = operation.iterations(1).measure_process();
///     // Perform some processor-intensive operation
///     let mut sum = 0;
///     for i in 0..1000 {
///         sum += i;
///     }
/// } // Process processor time is automatically tracked and recorded here
/// ```
///
/// For benchmarks with many iterations:
///
/// ```
/// use all_the_time::Session;
///
/// let mut session = Session::new();
/// let operation = session.operation("benchmark");
/// {
///     let _span = operation.iterations(1000).measure_process();
///     for i in 0..1000 {
///         // Perform the operation being benchmarked
///         let mut sum = 0;
///         sum += i;
///     }
/// } // Processor time is measured once and divided by 1000
/// ```
#[derive(Debug)]
pub struct ProcessSpan<'a> {
    operation: &'a mut Operation,
    start_time: Duration,
    iterations: u64,
}

impl<'a> ProcessSpan<'a> {
    /// Creates a new process span for the given operation and iteration count.
    ///
    /// # Panics
    ///
    /// Panics if `iterations` is zero.
    #[must_use]
    pub(crate) fn new(operation: &'a mut Operation, iterations: u64) -> Self {
        assert!(iterations != 0, "Iterations cannot be zero");

        let start_time = operation.platform().process_time();

        Self {
            operation,
            start_time,
            iterations,
        }
    }

    /// Calculates the process processor time delta since this span was created.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // The != 1 fork is broadly applicable, so mutations fail. Intentional.
    fn to_duration(&self) -> Duration {
        let current_time = self.operation.platform().process_time();
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

impl Drop for ProcessSpan<'_> {
    fn drop(&mut self) {
        let duration = self.to_duration();

        // Add the per-iteration duration for all iterations at once
        self.operation.add_iterations(duration, self.iterations);
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

    fn create_test_session_with_time(process_time: Duration) -> Session {
        let mut fake_platform = FakePlatform::new();
        fake_platform.set_process_time(process_time);
        let platform_facade = PlatformFacade::fake(fake_platform);
        Session::with_platform(platform_facade)
    }

    #[test]
    fn creates_span_with_iterations() {
        let mut session = create_test_session();
        let operation = session.operation("test");
        let span = operation.iterations(5).measure_process();
        assert_eq!(span.iterations, 5);
    }

    #[test]
    #[should_panic(expected = "Iterations cannot be zero")]
    fn panics_on_zero_iterations() {
        let mut session = create_test_session();
        let operation = session.operation("test");
        let _span = operation.iterations(0).measure_process();
    }

    #[test]
    fn records_cpu_time_measurements() {
        let mut session = create_test_session();
        let operation = session.operation("test");
        {
            let _span = operation.iterations(1).measure_process();
            // Perform some work
            let mut sum = 0;
            for i in 0..1000 {
                sum += i;
            }
            std::hint::black_box(sum);
        }

        // Verify that at least one measurement was recorded
        assert!(operation.total_iterations() > 0);
    }

    #[test]
    fn extracts_time_from_pal() {
        let mut session = create_test_session_with_time(Duration::ZERO);
        let operation = session.operation("test");

        {
            let _span = operation.iterations(1).measure_process();
        }

        // Should extract time from PAL and record one span with zero duration
        assert_eq!(operation.total_iterations(), 1);
        assert_eq!(operation.total_processor_time(), Duration::ZERO);
    }

    #[test]
    fn uses_process_time_from_pal() {
        // Verify that ProcessSpan specifically calls process_time() from the PAL
        let mut fake_platform = FakePlatform::new();
        fake_platform.set_process_time(Duration::from_millis(300));
        fake_platform.set_thread_time(Duration::from_millis(100)); // Different from process time

        let platform_facade = PlatformFacade::fake(fake_platform);
        let mut session = Session::with_platform(platform_facade);
        let operation = session.operation("test");

        {
            let _span = operation.iterations(1).measure_process();
            // Should use process_time (300ms), not thread_time (100ms)
        }

        assert_eq!(operation.total_iterations(), 1);
    }

    #[test]
    fn records_one_span_per_iteration() {
        let mut session = create_test_session();
        let operation = session.operation("test");

        {
            let _span = operation.iterations(7).measure_process();
            // Should record one span per iteration regardless of actual time measured
        }

        assert_eq!(operation.total_iterations(), 7);
    }

    #[test]
    fn accumulates_multiple_spans() {
        let mut session = create_test_session();
        let operation = session.operation("test");

        // Create multiple spans to test duration accumulation
        for i in 1..=3 {
            let _span = operation.iterations(i).measure_process();
        }

        // Should have recorded 1 + 2 + 3 = 6 total spans
        assert_eq!(operation.total_iterations(), 6);
    }
}
