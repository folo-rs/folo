//! Process-wide processor-time measurement span.

use std::cell::Cell;
use std::marker::PhantomData;
use std::panic::RefUnwindSafe;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::pal::{Platform, PlatformFacade};
use crate::{ERR_POISONED_LOCK, Operation, OperationMetrics};

/// A measurement of process-wide processor time over the span's lifetime.
///
/// Returned by [`Operation::measure_process`](crate::Operation::measure_process).
/// It captures the process's processor-time clock at creation and records the
/// elapsed delta when it is dropped, so the measured work should live inside the
/// span's scope.
///
/// Before the span is dropped the caller must state how many iterations the
/// measured work covers by calling [`iterations`](Self::iterations). Dropping a
/// span without an iteration count **panics**, because a measurement with no
/// iteration count is a programming error. If the thread is already unwinding
/// from a panic when the span drops, it records nothing and does not panic again,
/// leaving the original panic to propagate.
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
/// fn bench(c: &mut Criterion) {
///     let session = Session::new();
///     let operation = session.operation("hash_key");
///     c.bench_function("hash_key", |b| {
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
#[derive(Debug)]
#[must_use = "a span must be held across the measured work and given a count with `.iterations(n)`; it records when dropped and panics if the count is missing"]
pub struct ProcessSpan {
    metrics: Arc<Mutex<OperationMetrics>>,
    platform: PlatformFacade,
    start_time: Duration,
    iterations: Option<u64>,
    // Cell<()> is natively Send + !Sync, which opts the type out of Sync without requiring
    // an unsafe impl Send. Using PhantomData<*mut ()> + unsafe impl Send would be simpler
    // but triggers a rustc bug (rust-lang/rust#110338) in async generator Send inference.
    // We use the Cell<()> pattern here for consistency with the rest of the workspace.
    _not_sync: PhantomData<Cell<()>>,
}

// The Cell<()> marker is zero-sized with no actual mutable state, so there is nothing to
// observe in an inconsistent state during unwind.
impl RefUnwindSafe for ProcessSpan {}

impl ProcessSpan {
    pub(crate) fn new(operation: &Operation) -> Self {
        let platform = operation.platform().clone();
        let start_time = platform.process_time();

        Self {
            metrics: operation.metrics(),
            platform,
            start_time,
            iterations: None,
            _not_sync: PhantomData,
        }
    }

    /// Sets how many iterations the measured work covers.
    ///
    /// This must be called before the span is dropped. Pass the number of times the
    /// measured region repeats the work, or `1` for a single unit of work.
    ///
    /// Passing `0` — for example when a benchmark could not execute its workload —
    /// is permitted; the operation then reports a `NaN` per-iteration figure to
    /// signal that no valid measurement was produced.
    pub fn iterations(mut self, iterations: u64) -> Self {
        self.iterations = Some(iterations);
        self
    }
}

impl Drop for ProcessSpan {
    fn drop(&mut self) {
        // A panic while the span is held records nothing; panicking again here would
        // abort the process.
        if std::thread::panicking() {
            return;
        }

        let iterations = self.iterations.expect(
            "the span was dropped without an iteration count; call `.iterations(1)` \
             if the measured region is a single iteration",
        );
        let total_nanos = measured_nanos(&self.platform, self.start_time);
        let mut data = self.metrics.lock().expect(ERR_POISONED_LOCK);
        data.add_span(iterations, total_nanos);
    }
}

/// Total process processor time consumed since a span's start clock.
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

    fn create_test_session() -> (Session, FakePlatform) {
        let fake_platform = FakePlatform::new();
        let platform_facade = PlatformFacade::fake(fake_platform.clone());
        (Session::with_platform(platform_facade), fake_platform)
    }

    #[test]
    fn iterations_zero_is_accepted() {
        // A workload that could not run reports zero iterations; this records
        // rather than panicking, so the harness survives a failed benchmark.
        let (session, clock) = create_test_session();
        let operation = session.operation("test");

        let span = operation.measure_process().iterations(0);
        clock.set_process_time(Duration::from_nanos(17));
        drop(span);

        assert_eq!(operation.total_iterations(), 0);
        assert_eq!(operation.total_processor_time(), Duration::from_nanos(17));
        let report = session.to_report();
        let (_, operation) = report.operations().next().unwrap();
        assert_eq!(operation.statistics().unwrap().span_count, 1);
        assert!(operation.statistics().unwrap().slope_nanos.is_nan());
        assert_eq!(operation.processor_time(), None);
    }

    #[test]
    fn records_span_via_iterations_guard() {
        let (session, clock) = create_test_session();
        let operation = session.operation("test");

        // Nonzero opening and closing samples distinguish the delta from either clock reading.
        clock.set_process_time(Duration::from_nanos(100));
        {
            let _span = operation.measure_process().iterations(3);
            clock.set_process_time(Duration::from_nanos(163));
            assert_eq!(operation.total_iterations(), 0);
            assert_eq!(operation.total_processor_time(), Duration::ZERO);
        }

        assert_eq!(operation.total_iterations(), 3);
        assert_eq!(operation.total_processor_time(), Duration::from_nanos(63));
    }

    #[test]
    fn records_span_via_post_hoc_iterations() {
        let (session, clock) = create_test_session();
        let operation = session.operation("test");

        let span = operation.measure_process();
        clock.set_process_time(Duration::from_nanos(77));
        drop(span.iterations(7));

        assert_eq!(operation.total_iterations(), 7);
        assert_eq!(operation.total_processor_time(), Duration::from_nanos(77));
    }

    #[test]
    #[should_panic]
    fn dropping_span_without_iterations_panics() {
        let (session, _) = create_test_session();
        let operation = session.operation("test");

        drop(operation.measure_process());
    }

    #[test]
    fn panic_while_held_records_nothing() {
        let (session, clock) = create_test_session();
        let operation = session.operation("test");

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _span = operation.measure_process().iterations(1);
            clock.set_process_time(Duration::from_nanos(19));
            panic!("boom");
        }));

        assert!(result.is_err());
        assert_eq!(operation.total_iterations(), 0);
        assert_eq!(operation.total_processor_time(), Duration::ZERO);
    }

    #[test]
    fn uses_process_time_from_pal() {
        let (session, clock) = create_test_session();
        // Distinct starts and deltas detect mixing the thread and process clocks at either end.
        clock.set_process_time(Duration::from_nanos(300));
        clock.set_thread_time(Duration::from_nanos(100));
        let operation = session.operation("test");

        {
            let _span = operation.measure_process().iterations(1);
            clock.set_process_time(Duration::from_nanos(390));
            clock.set_thread_time(Duration::from_nanos(120));
        }

        assert_eq!(operation.total_iterations(), 1);
        assert_eq!(operation.total_processor_time(), Duration::from_nanos(90));
    }

    #[test]
    fn unchanged_process_clock_records_zero() {
        let (session, clock) = create_test_session();
        clock.set_process_time(Duration::from_nanos(23));
        let operation = session.operation("test");

        drop(operation.measure_process().iterations(2));

        assert_eq!(operation.total_iterations(), 2);
        assert_eq!(operation.total_processor_time(), Duration::ZERO);
    }

    #[test]
    fn decreasing_process_clock_saturates_to_zero() {
        let (session, clock) = create_test_session();
        clock.set_process_time(Duration::from_nanos(23));
        let operation = session.operation("test");
        let span = operation.measure_process().iterations(2);
        clock.set_process_time(Duration::from_nanos(7));
        drop(span);

        assert_eq!(operation.total_iterations(), 2);
        assert_eq!(operation.total_processor_time(), Duration::ZERO);
    }

    #[test]
    fn process_delta_saturates_at_nanosecond_capacity() {
        let (session, clock) = create_test_session();
        let operation = session.operation("test");
        let span = operation.measure_process().iterations(1);
        // One nanosecond beyond the stored span capacity exercises the conversion fallback.
        clock.set_process_time(Duration::from_nanos_u128(u128::from(u64::MAX) + 1));
        drop(span);

        assert_eq!(
            operation.total_processor_time(),
            Duration::from_nanos(u64::MAX)
        );
        assert_eq!(operation.total_iterations(), 1);
    }

    #[test]
    fn accumulates_multiple_spans() {
        let (session, clock) = create_test_session();
        let operation = session.operation("test");

        // Unequal batches preserve whole-span totals without per-span integer division.
        clock.set_process_time(Duration::from_nanos(100));
        {
            let _span = operation.measure_process().iterations(2);
            clock.set_process_time(Duration::from_nanos(107));
        }
        {
            let _span = operation.measure_process().iterations(3);
            clock.set_process_time(Duration::from_nanos(118));
        }

        assert_eq!(operation.total_iterations(), 5);
        assert_eq!(operation.total_processor_time(), Duration::from_nanos(18));
        let report = session.to_report();
        let (_, operation) = report.operations().next().unwrap();
        assert_eq!(operation.statistics().unwrap().span_count, 2);
    }

    // Static assertions for thread safety.
    // The span is Send but !Sync due to PhantomData<Cell<()>>.
    static_assertions::assert_impl_all!(super::ProcessSpan: Send, UnwindSafe, RefUnwindSafe);
    static_assertions::assert_not_impl_any!(super::ProcessSpan: Sync);
}
