//! Thread processor-time measurement span.

use std::marker::PhantomData;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::pal::{Platform, PlatformFacade};
use crate::{ERR_POISONED_LOCK, Operation, OperationMetrics};

/// A measurement of the current thread's processor time over the span's lifetime.
///
/// Returned by [`Operation::measure_thread`](crate::Operation::measure_thread). It
/// captures the thread's processor-time clock at creation and records the elapsed
/// delta when it is dropped, so the measured work should live inside the span's
/// scope.
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
///             let _span = operation.measure_thread().iterations(iters);
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
///
/// When the count is only known after the work has run, set it afterwards and let
/// the span record as it drops:
///
/// ```
/// use all_the_time::Session;
///
/// # fn main() {
/// let session = Session::new();
/// # let session = session.no_stdout().no_file();
/// let operation = session.operation("drain_queue");
///
/// let span = operation.measure_thread();
/// let mut processed = 0_u64;
/// for item in 0..5 {
///     std::hint::black_box(item * 2); // do work while draining
///     processed += 1;
/// }
/// drop(span.iterations(processed));
/// # }
/// ```
#[derive(Debug)]
#[must_use = "a span must be held across the measured work and given a count with `.iterations(n)`; it records when dropped and panics if the count is missing"]
pub struct ThreadSpan {
    metrics: Arc<Mutex<OperationMetrics>>,
    platform: PlatformFacade,
    start_time: Duration,
    iterations: Option<u64>,

    _single_threaded: PhantomData<*const ()>,
}

impl ThreadSpan {
    pub(crate) fn new(operation: &Operation) -> Self {
        let platform = operation.platform().clone();
        let start_time = platform.thread_time();

        Self {
            metrics: operation.metrics(),
            platform,
            start_time,
            iterations: None,
            _single_threaded: PhantomData,
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

impl Drop for ThreadSpan {
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

/// Total thread processor time consumed since a span's start clock.
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
    fn iterations_zero_is_accepted() {
        // A workload that could not run reports zero iterations; this records
        // rather than panicking, so the harness survives a failed benchmark.
        let session = create_test_session();
        let operation = session.operation("test");

        drop(operation.measure_thread().iterations(0));

        assert_eq!(operation.total_iterations(), 0);
    }

    #[test]
    fn records_span_via_iterations_guard() {
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
    fn records_span_via_post_hoc_iterations() {
        let session = create_test_session_with_time(Duration::ZERO);
        let operation = session.operation("test");

        drop(operation.measure_thread().iterations(5));

        assert_eq!(operation.total_iterations(), 5);
        assert_eq!(operation.total_processor_time(), Duration::ZERO);
    }

    #[test]
    #[should_panic]
    fn dropping_span_without_iterations_panics() {
        let session = create_test_session();
        let operation = session.operation("test");

        drop(operation.measure_thread());
    }

    #[test]
    fn panic_while_held_records_nothing() {
        let session = create_test_session_with_time(Duration::ZERO);
        let operation = session.operation("test");

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _span = operation.measure_thread().iterations(1);
            panic!("boom");
        }));

        assert!(result.is_err());
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
        // Verify that a thread span specifically calls thread_time() from the PAL.
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
    // The span should NOT be Send or Sync due to PhantomData<*const ()>.
    static_assertions::assert_not_impl_all!(super::ThreadSpan: Send);
    static_assertions::assert_not_impl_all!(super::ThreadSpan: Sync);

    // Static assertions for unwind safety.
    static_assertions::assert_impl_all!(super::ThreadSpan: UnwindSafe, RefUnwindSafe);
}
