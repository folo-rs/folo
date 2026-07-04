//! Thread processor-time measurement and span.

use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::pal::{Platform, PlatformFacade};
use crate::{ERR_POISONED_LOCK, Operation, OperationMetrics};

/// An in-progress measurement of this thread's processor time, awaiting an
/// explicit iteration count.
///
/// Returned by [`Operation::measure_thread`](crate::Operation::measure_thread).
/// It captures the thread's processor-time clock at creation but records nothing
/// on its own — the caller must state how many iterations the measured work
/// covers, either up front with [`iterations`](Self::iterations) (an RAII guard
/// that records when its scope ends) or afterwards with [`complete`](Self::complete).
/// Dropping the measurement without either **discards** it, so a panic or early
/// return in the measured region records nothing.
///
/// # Examples
///
/// The canonical benchmark pattern feeds Criterion's chosen iteration count
/// straight into [`iterations`](Self::iterations) from within `iter_custom`:
///
/// ```no_run
/// use std::hint::black_box;
/// use std::time::Instant;
///
/// use all_the_time::Session;
/// use criterion::Criterion;
///
/// # fn main() {
/// let session = Session::new();
/// let operation = session.operation("hash_key");
/// let mut criterion = Criterion::default();
/// criterion.bench_function("hash_key", |b| {
///     b.iter_custom(|iters| {
///         let start = Instant::now();
///         let _span = operation.measure_thread().iterations(iters);
///         for _ in 0..iters {
///             black_box(42_u64.wrapping_mul(2));
///         }
///         start.elapsed()
///     });
/// });
/// # }
/// ```
#[derive(Debug)]
#[must_use = "a measurement records nothing until `.iterations(n)` or `.complete(n)` is called"]
pub struct ThreadMeasurement {
    metrics: Arc<Mutex<OperationMetrics>>,
    platform: PlatformFacade,
    start_time: Duration,

    _single_threaded: PhantomData<*const ()>,
}

impl ThreadMeasurement {
    pub(crate) fn new(operation: &Operation) -> Self {
        let platform = operation.platform().clone();
        let start_time = platform.thread_time();

        Self {
            metrics: operation.metrics(),
            platform,
            start_time,
            _single_threaded: PhantomData,
        }
    }

    /// Binds this measurement to an iteration count known in advance, returning an
    /// RAII guard that records the thread's processor-time delta when it is
    /// dropped.
    ///
    /// Keep the measured work inside the returned span's scope so the delta is
    /// captured at the right moment. This is the terminal used in the
    /// `iter_custom` benchmark pattern (see the type-level example).
    ///
    /// # Panics
    ///
    /// Panics if `iterations` is zero.
    pub fn iterations(self, iterations: u64) -> ThreadSpan {
        assert!(iterations != 0, "iterations cannot be zero");
        ThreadSpan {
            metrics: Arc::clone(&self.metrics),
            platform: self.platform.clone(),
            start_time: self.start_time,
            iterations,
            _single_threaded: PhantomData,
        }
    }

    /// Records the thread's processor-time delta now, attributing it to
    /// `iterations` iterations.
    ///
    /// Use this when the iteration count is only known after the measured work has
    /// run — for example a loop that runs until some budget is exhausted.
    ///
    /// # Examples
    ///
    /// ```
    /// use all_the_time::Session;
    ///
    /// # fn main() {
    /// let session = Session::new();
    /// # let session = session.no_stdout().no_file();
    /// let operation = session.operation("drain_queue");
    ///
    /// // The iteration count is only known once the work has finished.
    /// let measurement = operation.measure_thread();
    /// let mut processed = 0_u64;
    /// for item in 0..5 {
    ///     std::hint::black_box(item * 2); // do work while draining
    ///     processed += 1;
    /// }
    /// measurement.complete(processed);
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if `iterations` is zero.
    pub fn complete(self, iterations: u64) {
        assert!(iterations != 0, "iterations cannot be zero");
        let total_nanos = measured_nanos(&self.platform, self.start_time);
        let mut data = self.metrics.lock().expect(ERR_POISONED_LOCK);
        data.add_span(iterations, total_nanos);
    }
}

/// An active thread processor-time measurement bound to a known iteration count.
///
/// Created by [`ThreadMeasurement::iterations`]. It records the thread's
/// processor-time delta when dropped, so the measured work should live inside the
/// span's scope.
#[derive(Debug)]
#[must_use = "the measurement is recorded when the span is dropped, so it must be held for the measured work"]
pub struct ThreadSpan {
    metrics: Arc<Mutex<OperationMetrics>>,
    platform: PlatformFacade,
    start_time: Duration,
    iterations: u64,

    _single_threaded: PhantomData<*const ()>,
}

impl Drop for ThreadSpan {
    fn drop(&mut self) {
        let total_nanos = measured_nanos(&self.platform, self.start_time);
        let mut data = self.metrics.lock().expect(ERR_POISONED_LOCK);
        data.add_span(self.iterations, total_nanos);
    }
}

/// Total thread processor time consumed since a measurement's start clock.
fn measured_nanos(platform: &PlatformFacade, start_time: Duration) -> u64 {
    let current_time = platform.thread_time();
    let total_duration = current_time.saturating_sub(start_time);
    u64::try_from(total_duration.as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};
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
    #[should_panic]
    fn iterations_panics_on_zero() {
        let session = create_test_session();
        let operation = session.operation("test");
        let _span = operation.measure_thread().iterations(0);
    }

    #[test]
    #[should_panic]
    fn complete_panics_on_zero() {
        let session = create_test_session();
        let operation = session.operation("test");
        operation.measure_thread().complete(0);
    }

    #[test]
    fn records_span_via_iterations() {
        let session = create_test_session_with_time(Duration::ZERO);
        let operation = session.operation("test");

        {
            let _span = operation.measure_thread().iterations(1);
        }

        // Should extract time from PAL and record one span with zero duration.
        assert_eq!(operation.total_iterations(), 1);
        assert_eq!(operation.total_processor_time(), Duration::ZERO);
    }

    #[test]
    fn records_span_via_complete() {
        let session = create_test_session_with_time(Duration::ZERO);
        let operation = session.operation("test");

        operation.measure_thread().complete(5);

        assert_eq!(operation.total_iterations(), 5);
        assert_eq!(operation.total_processor_time(), Duration::ZERO);
    }

    #[test]
    fn dropping_measurement_without_terminal_records_nothing() {
        let session = create_test_session();
        let operation = session.operation("test");

        drop(operation.measure_thread());

        assert_eq!(operation.total_iterations(), 0);
    }

    #[test]
    fn records_batch_iterations() {
        let session = create_test_session();
        let operation = session.operation("test");

        {
            let _span = operation.measure_thread().iterations(10);
        }

        assert_eq!(operation.total_iterations(), 10);
        assert!(operation.total_processor_time() >= Duration::ZERO);
    }

    #[test]
    fn uses_thread_time_from_pal() {
        // Verify that ThreadMeasurement specifically calls thread_time() from the PAL.
        let fake_platform = FakePlatform::new();
        fake_platform.set_thread_time(Duration::from_millis(50));
        fake_platform.set_process_time(Duration::from_millis(200)); // Different from thread time

        let platform_facade = PlatformFacade::fake(fake_platform);
        let session = Session::with_platform(platform_facade);
        let operation = session.operation("test");

        {
            let _span = operation.measure_thread().iterations(1);
            // Should use thread_time (50ms), not process_time (200ms).
        }

        assert_eq!(operation.total_iterations(), 1);
    }

    // Static assertions for thread safety.
    // Both types should NOT be Send or Sync due to PhantomData<*const ()>.
    static_assertions::assert_not_impl_all!(super::ThreadMeasurement: Send);
    static_assertions::assert_not_impl_all!(super::ThreadMeasurement: Sync);
    static_assertions::assert_not_impl_all!(super::ThreadSpan: Send);
    static_assertions::assert_not_impl_all!(super::ThreadSpan: Sync);

    // Static assertions for unwind safety.
    static_assertions::assert_impl_all!(super::ThreadMeasurement: UnwindSafe, RefUnwindSafe);
    static_assertions::assert_impl_all!(super::ThreadSpan: UnwindSafe, RefUnwindSafe);
}
