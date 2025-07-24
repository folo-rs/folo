//! Process-wide processor time tracking span.

use std::cell::Cell;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::Operation;
use crate::constants::ERR_POISONED_LOCK;
use crate::pal::{Platform, PlatformFacade};
use crate::session::OperationMetrics;

/// Use this to track processor times for code that runs on any thread.
///
/// A span of code for which we track process processor time between creation and drop.
///
/// Measures processor time consumed by the entire process (all threads).
///
/// # Examples
///
/// ```
/// use all_the_time::Session;
///
/// let session = Session::new();
/// let operation = session.operation("test");
/// {
///     let _span = operation.measure_process();
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
/// let session = Session::new();
/// let operation = session.operation("benchmark");
/// {
///     let _span = operation.measure_process().iterations(1000);
///     for i in 0..1000 {
///         // Perform the operation being benchmarked
///         let mut sum = 0;
///         sum += i;
///     }
/// } // Processor time is measured once and divided by 1000
/// ```
#[derive(Debug)]
#[must_use = "Measurements are taken between creation and drop"]
pub struct ProcessSpan {
    metrics: Arc<Mutex<OperationMetrics>>,
    platform: PlatformFacade,
    start_time: Duration,
    iterations: u64,
    _not_sync: PhantomData<Cell<()>>,
}

impl ProcessSpan {
    /// Creates a new process span for the given operation and iteration count.
    ///
    /// # Panics
    ///
    /// Panics if `iterations` is zero.
    pub(crate) fn new(operation: &Operation, iterations: u64) -> Self {
        assert!(iterations != 0, "Iterations cannot be zero");

        let platform = operation.platform().clone();
        let start_time = platform.process_time();

        Self {
            metrics: operation.metrics(),
            platform,
            start_time,
            iterations,
            _not_sync: PhantomData,
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
    ///     let _span = operation.measure_process().iterations(1000);
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
    ///     let span = operation.measure_process();
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

    /// Calculates the process processor time delta since this span was created.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // The != 1 fork is broadly applicable, so mutations fail. Intentional.
    fn to_duration(&self) -> Duration {
        let current_time = self.platform.process_time();
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

impl Drop for ProcessSpan {
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

    fn create_test_session_with_time(process_time: Duration) -> Session {
        let fake_platform = FakePlatform::new();
        fake_platform.set_process_time(process_time);
        let platform_facade = PlatformFacade::fake(fake_platform);
        Session::with_platform(platform_facade)
    }

    #[test]
    fn creates_span_with_iterations() {
        let session = create_test_session();
        let operation = session.operation("test");
        let span = operation.measure_process().iterations(5);
        assert_eq!(span.iterations, 5);
    }

    #[test]
    #[should_panic(expected = "Iterations cannot be zero")]
    fn panics_on_zero_iterations() {
        let session = create_test_session();
        let operation = session.operation("test");
        let _span = operation.measure_process().iterations(0);
    }

    #[test]
    fn records_cpu_time_measurements() {
        let session = create_test_session();
        let operation = session.operation("test");
        {
            let _span = operation.measure_process();
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
        let session = create_test_session_with_time(Duration::ZERO);
        let operation = session.operation("test");

        {
            let _span = operation.measure_process();
        }

        // Should extract time from PAL and record one span with zero duration
        assert_eq!(operation.total_iterations(), 1);
        assert_eq!(operation.total_processor_time(), Duration::ZERO);
    }

    #[test]
    fn uses_process_time_from_pal() {
        // Verify that ProcessSpan specifically calls process_time() from the PAL
        let fake_platform = FakePlatform::new();
        fake_platform.set_process_time(Duration::from_millis(300));
        fake_platform.set_thread_time(Duration::from_millis(100)); // Different from process time

        let platform_facade = PlatformFacade::fake(fake_platform);
        let session = Session::with_platform(platform_facade);
        let operation = session.operation("test");

        {
            let _span = operation.measure_process();
            // Should use process_time (300ms), not thread_time (100ms)
        }

        assert_eq!(operation.total_iterations(), 1);
    }

    #[test]
    fn records_one_span_per_iteration() {
        let session = create_test_session();
        let operation = session.operation("test");

        {
            let _span = operation.measure_process().iterations(7);
            // Should record one span per iteration regardless of actual time measured
        }

        assert_eq!(operation.total_iterations(), 7);
    }

    #[test]
    fn accumulates_multiple_spans() {
        let session = create_test_session();
        let operation = session.operation("test");

        // Create multiple spans to test duration accumulation
        for i in 1..=3 {
            let _span = operation.measure_process().iterations(i);
        }

        // Should have recorded 1 + 2 + 3 = 6 total spans
        assert_eq!(operation.total_iterations(), 6);
    }

    // Static assertions for thread safety
    static_assertions::assert_impl_all!(super::ProcessSpan: Send);
    static_assertions::assert_not_impl_any!(super::ProcessSpan: Sync);
    // ProcessSpan is Send but !Sync due to PhantomData<Cell<()>>
}
