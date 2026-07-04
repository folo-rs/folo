//! Process-wide processor-time measurement and span.

use std::cell::Cell;
use std::marker::PhantomData;
use std::panic::RefUnwindSafe;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::pal::{Platform, PlatformFacade};
use crate::{ERR_POISONED_LOCK, Operation, OperationMetrics};

/// An in-progress measurement of process-wide processor time, awaiting an
/// explicit iteration count.
///
/// Returned by [`Operation::measure_process`](crate::Operation::measure_process).
/// It captures the process's processor-time clock at creation but records nothing
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
///         let _span = operation.measure_process().iterations(iters);
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
pub struct ProcessMeasurement {
    metrics: Arc<Mutex<OperationMetrics>>,
    platform: PlatformFacade,
    start_time: Duration,
    // Cell<()> is natively Send + !Sync, which opts the type out of Sync without requiring
    // an unsafe impl Send. Using PhantomData<*mut ()> + unsafe impl Send would be simpler
    // but triggers a rustc bug (rust-lang/rust#110338) in async generator Send inference.
    // We use the Cell<()> pattern here for consistency with the rest of the workspace.
    _not_sync: PhantomData<Cell<()>>,
}

// The Cell<()> marker is zero-sized with no actual mutable state, so there is nothing to
// observe in an inconsistent state during unwind.
impl RefUnwindSafe for ProcessMeasurement {}

impl ProcessMeasurement {
    pub(crate) fn new(operation: &Operation) -> Self {
        let platform = operation.platform().clone();
        let start_time = platform.process_time();

        Self {
            metrics: operation.metrics(),
            platform,
            start_time,
            _not_sync: PhantomData,
        }
    }

    /// Binds this measurement to an iteration count known in advance, returning an
    /// RAII guard that records the process's processor-time delta when it is
    /// dropped.
    ///
    /// Keep the measured work inside the returned span's scope so the delta is
    /// captured at the right moment. This is the terminal used in the
    /// `iter_custom` benchmark pattern (see the type-level example).
    ///
    /// # Panics
    ///
    /// Panics if `iterations` is zero.
    pub fn iterations(self, iterations: u64) -> ProcessSpan {
        assert!(iterations != 0, "iterations cannot be zero");
        ProcessSpan {
            metrics: Arc::clone(&self.metrics),
            platform: self.platform.clone(),
            start_time: self.start_time,
            iterations,
            _not_sync: PhantomData,
        }
    }

    /// Records the process's processor-time delta now, attributing it to
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
    /// let measurement = operation.measure_process();
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

/// An active process-wide processor-time measurement bound to a known iteration
/// count.
///
/// Created by [`ProcessMeasurement::iterations`]. It records the process's
/// processor-time delta when dropped, so the measured work should live inside the
/// span's scope.
#[derive(Debug)]
#[must_use = "the measurement is recorded when the span is dropped, so it must be held for the measured work"]
pub struct ProcessSpan {
    metrics: Arc<Mutex<OperationMetrics>>,
    platform: PlatformFacade,
    start_time: Duration,
    iterations: u64,
    _not_sync: PhantomData<Cell<()>>,
}

// The Cell<()> marker is zero-sized with no actual mutable state, so there is nothing to
// observe in an inconsistent state during unwind.
impl RefUnwindSafe for ProcessSpan {}

impl Drop for ProcessSpan {
    fn drop(&mut self) {
        let total_nanos = measured_nanos(&self.platform, self.start_time);
        let mut data = self.metrics.lock().expect(ERR_POISONED_LOCK);
        data.add_span(self.iterations, total_nanos);
    }
}

/// Total process processor time consumed since a measurement's start clock.
fn measured_nanos(platform: &PlatformFacade, start_time: Duration) -> u64 {
    let current_time = platform.process_time();
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

    fn create_test_session_with_time(process_time: Duration) -> Session {
        let fake_platform = FakePlatform::new();
        fake_platform.set_process_time(process_time);
        let platform_facade = PlatformFacade::fake(fake_platform);
        Session::with_platform(platform_facade)
    }

    #[test]
    #[should_panic]
    fn iterations_panics_on_zero() {
        let session = create_test_session();
        let operation = session.operation("test");
        let _span = operation.measure_process().iterations(0);
    }

    #[test]
    #[should_panic]
    fn complete_panics_on_zero() {
        let session = create_test_session();
        let operation = session.operation("test");
        operation.measure_process().complete(0);
    }

    #[test]
    fn records_span_via_iterations() {
        let session = create_test_session_with_time(Duration::ZERO);
        let operation = session.operation("test");

        {
            let _span = operation.measure_process().iterations(1);
        }

        assert_eq!(operation.total_iterations(), 1);
        assert_eq!(operation.total_processor_time(), Duration::ZERO);
    }

    #[test]
    fn records_span_via_complete() {
        let session = create_test_session_with_time(Duration::ZERO);
        let operation = session.operation("test");

        operation.measure_process().complete(7);

        assert_eq!(operation.total_iterations(), 7);
        assert_eq!(operation.total_processor_time(), Duration::ZERO);
    }

    #[test]
    fn dropping_measurement_without_terminal_records_nothing() {
        let session = create_test_session();
        let operation = session.operation("test");

        drop(operation.measure_process());

        assert_eq!(operation.total_iterations(), 0);
    }

    #[test]
    fn uses_process_time_from_pal() {
        // Verify that ProcessMeasurement specifically calls process_time() from the PAL.
        let fake_platform = FakePlatform::new();
        fake_platform.set_process_time(Duration::from_millis(300));
        fake_platform.set_thread_time(Duration::from_millis(100)); // Different from process time

        let platform_facade = PlatformFacade::fake(fake_platform);
        let session = Session::with_platform(platform_facade);
        let operation = session.operation("test");

        {
            let _span = operation.measure_process().iterations(1);
            // Should use process_time (300ms), not thread_time (100ms).
        }

        assert_eq!(operation.total_iterations(), 1);
    }

    #[test]
    fn accumulates_multiple_spans() {
        let session = create_test_session();
        let operation = session.operation("test");

        for i in 1..=3 {
            let _span = operation.measure_process().iterations(i);
        }

        // Should have recorded 1 + 2 + 3 = 6 total iterations.
        assert_eq!(operation.total_iterations(), 6);
    }

    // Static assertions for thread safety.
    // Both types are Send but !Sync due to PhantomData<Cell<()>>.
    static_assertions::assert_impl_all!(super::ProcessMeasurement: Send, UnwindSafe, RefUnwindSafe);
    static_assertions::assert_not_impl_any!(super::ProcessMeasurement: Sync);
    static_assertions::assert_impl_all!(super::ProcessSpan: Send, UnwindSafe, RefUnwindSafe);
    static_assertions::assert_not_impl_any!(super::ProcessSpan: Sync);
}
