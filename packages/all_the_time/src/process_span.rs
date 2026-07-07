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
/// iteration count is a programming error. A panic or early return while the span
/// is held records nothing.
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
#[must_use = "a span records its measurement when dropped, so it must be held for the measured work"]
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
    /// # Panics
    ///
    /// Panics if `iterations` is zero.
    pub fn iterations(mut self, iterations: u64) -> Self {
        assert!(iterations != 0, "iterations cannot be zero");
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
    fn records_span_via_iterations_guard() {
        let session = create_test_session_with_time(Duration::ZERO);
        let operation = session.operation("test");

        {
            let _span = operation.measure_process().iterations(1);
        }

        assert_eq!(operation.total_iterations(), 1);
        assert_eq!(operation.total_processor_time(), Duration::ZERO);
    }

    #[test]
    fn records_span_via_post_hoc_iterations() {
        let session = create_test_session_with_time(Duration::ZERO);
        let operation = session.operation("test");

        drop(operation.measure_process().iterations(7));

        assert_eq!(operation.total_iterations(), 7);
        assert_eq!(operation.total_processor_time(), Duration::ZERO);
    }

    #[test]
    #[should_panic]
    fn dropping_span_without_iterations_panics() {
        let session = create_test_session();
        let operation = session.operation("test");

        drop(operation.measure_process());
    }

    #[test]
    fn panic_while_held_records_nothing() {
        let session = create_test_session_with_time(Duration::ZERO);
        let operation = session.operation("test");

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _span = operation.measure_process().iterations(1);
            panic!("boom");
        }));

        assert!(result.is_err());
        assert_eq!(operation.total_iterations(), 0);
    }

    #[test]
    fn uses_process_time_from_pal() {
        // Verify that a process span specifically calls process_time() from the PAL.
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
    // The span is Send but !Sync due to PhantomData<Cell<()>>.
    static_assertions::assert_impl_all!(super::ProcessSpan: Send, UnwindSafe, RefUnwindSafe);
    static_assertions::assert_not_impl_any!(super::ProcessSpan: Sync);
}
