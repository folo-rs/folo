//! Thread-local allocation tracking span.

use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use crate::allocator::{PerThreadCounters, get_or_init_thread_counters};
use crate::operation_metrics::SpanMeasurement;
use crate::{ERR_POISONED_LOCK, Operation, OperationMetrics};

/// A measurement of this thread's allocations over the span's lifetime.
///
/// Returned by [`Operation::measure_thread`](crate::Operation::measure_thread). It
/// captures the thread's allocation counters at creation and records the delta when
/// it is dropped, so the measured work should live inside the span's scope.
///
/// Before the span is dropped the caller must state how many iterations the
/// measured work covers by calling [`iterations`](Self::iterations). Dropping a
/// span without an iteration count **panics**, because a measurement with no
/// iteration count is a programming error. If the thread is already unwinding
/// from a panic when the span drops, it records nothing and does not panic again,
/// leaving the original panic to propagate.
///
/// Spans may nest, but overlapping spans on one thread must be dropped in reverse order
/// of creation, which holding each in a scoped binding achieves naturally.
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
/// use alloc_tracker::{Allocator, Session};
/// use criterion::Criterion;
///
/// #[global_allocator]
/// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
///
/// fn bench(c: &mut Criterion) {
///     let session = Session::new();
///     let operation = session.operation("allocate_buffer");
///     c.bench_function("allocate_buffer", |b| {
///         b.iter_custom(|iters| {
///             let start = Instant::now();
///             let _span = operation.measure_thread().iterations(iters);
///
///             for _ in 0..iters {
///                 black_box(vec![1_u8; 64]);
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
/// use alloc_tracker::{Allocator, Session};
///
/// #[global_allocator]
/// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
///
/// # fn main() {
/// let session = Session::new();
/// # let session = session.no_stdout().no_file();
/// let operation = session.operation("drain_queue");
///
/// let span = operation.measure_thread();
/// let mut processed = 0_u64;
/// for item in 0..5 {
///     let _data = vec![item; 8]; // allocate while draining
///     processed += 1;
/// }
/// drop(span.iterations(processed));
/// # }
/// ```
#[derive(Debug)]
#[must_use = "a span must be held across the measured work and given a count with `.iterations(n)`; it records when dropped and panics if the count is missing"]
pub struct ThreadSpan {
    metrics: Arc<Mutex<OperationMetrics>>,
    start_bytes: u64,
    start_count: u64,
    start_outstanding: i64,
    enclosing_peak: i64,
    iterations: Option<u64>,

    _single_threaded: PhantomData<*const ()>,
}

impl ThreadSpan {
    pub(crate) fn new(operation: &Operation) -> Self {
        let counters = get_or_init_thread_counters();
        let start_outstanding = counters.outstanding();

        // Rebase the watermark onto this span's entry level so that it goes on to measure
        // only what this span itself holds. The displaced value is handed back on drop,
        // which is what lets spans nest.
        let enclosing_peak = counters.peak_outstanding();
        counters.set_peak_outstanding(start_outstanding);

        Self {
            metrics: operation.metrics(),
            start_bytes: counters.bytes(),
            start_count: counters.count(),
            start_outstanding,
            enclosing_peak,
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
        let counters = get_or_init_thread_counters();

        // The watermark must go back to the enclosing span before any early exit below. A
        // span that is abandoned without recording would otherwise leave its own rebased
        // watermark in place and silently suppress the enclosing span's peak.
        let peak_bytes = restore_watermark(counters, self.start_outstanding, self.enclosing_peak);

        // A panic while the span is held records nothing; panicking again here would
        // abort the process.
        if std::thread::panicking() {
            return;
        }

        let iterations = self.iterations.expect(
            "the span was dropped without an iteration count; call `.iterations(1)` \
             if the measured region is a single iteration",
        );
        let (bytes_delta, count_delta) =
            thread_deltas(counters, self.start_bytes, self.start_count);
        let mut data = self.metrics.lock().expect(ERR_POISONED_LOCK);
        data.add_span(SpanMeasurement {
            iterations,
            bytes: bytes_delta,
            count: count_delta,
            peak_outstanding_bytes: Some(peak_bytes),
        });
    }
}

/// Hands the allocation watermark back to the enclosing span and reports how far above its
/// own entry level the closing span reached.
///
/// The enclosing span's watermark is the higher of the value it had on entry and the level
/// the inner span reached, because memory the inner span held was equally outstanding from
/// the enclosing span's point of view.
fn restore_watermark(
    counters: &PerThreadCounters,
    start_outstanding: i64,
    enclosing_peak: i64,
) -> u64 {
    let span_peak = counters.peak_outstanding();
    counters.set_peak_outstanding(enclosing_peak.max(span_peak));

    // The watermark was rebased to the entry level and only ever rises, so the difference is
    // already non-negative; the clamp states that invariant rather than correcting for it.
    span_peak
        .saturating_sub(start_outstanding)
        .max(0)
        .cast_unsigned()
}

/// Computes the thread's allocation deltas since a span's start counters.
///
/// The whole-span deltas are returned undivided; per-iteration figures are derived
/// later by the shared span accumulator, which weights each span by its iteration
/// count.
fn thread_deltas(counters: &PerThreadCounters, start_bytes: u64, start_count: u64) -> (u64, u64) {
    let bytes_delta = counters
        .bytes()
        .checked_sub(start_bytes)
        .expect("thread bytes allocated could not possibly decrease");
    let count_delta = counters
        .count()
        .checked_sub(start_count)
        .expect("thread allocations count could not possibly decrease");

    (bytes_delta, count_delta)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use super::*;
    use crate::Session;
    use crate::allocator::{register_fake_allocation, register_fake_deallocation};

    // Static assertions for thread safety.
    // The span should NOT be Send or Sync due to PhantomData<*const ()>.
    static_assertions::assert_not_impl_all!(ThreadSpan: Send);
    static_assertions::assert_not_impl_all!(ThreadSpan: Sync);

    // Static assertions for unwind safety.
    static_assertions::assert_impl_all!(ThreadSpan: UnwindSafe, RefUnwindSafe);

    #[test]
    fn peak_is_the_high_water_mark_not_the_total() {
        let session = Session::new().no_stdout().no_file();
        let operation = session.operation("test");

        {
            let _span = operation.measure_thread().iterations(1);

            // Three allocations of 100 that never coexist: 300 allocated, 100 held.
            for _ in 0..3 {
                register_fake_allocation(100, 1);
                register_fake_deallocation(100);
            }
        }

        assert_eq!(operation.total_bytes_allocated(), 300);
        assert_eq!(operation.peak_outstanding_bytes(), Some(100));
    }

    #[test]
    fn peak_ignores_memory_outstanding_before_the_span() {
        let session = Session::new().no_stdout().no_file();
        let operation = session.operation("test");

        // Memory the caller already holds is not the span's doing.
        register_fake_allocation(1000, 1);

        {
            let _span = operation.measure_thread().iterations(1);
            register_fake_allocation(50, 1);
        }

        register_fake_deallocation(1050);

        assert_eq!(operation.peak_outstanding_bytes(), Some(50));
    }

    #[test]
    fn peak_under_reports_when_the_span_frees_first() {
        // A documented limitation of measuring against the entry level: the span holds 500
        // bytes of its own but never pushes the outstanding total above where it started,
        // so it reports nothing.
        let session = Session::new().no_stdout().no_file();
        let operation = session.operation("test");

        register_fake_allocation(1000, 1);

        {
            let _span = operation.measure_thread().iterations(1);
            register_fake_deallocation(800);
            register_fake_allocation(500, 1);
        }

        register_fake_deallocation(700);

        assert_eq!(operation.peak_outstanding_bytes(), Some(0));
    }

    #[test]
    fn nested_span_peak_is_visible_to_the_enclosing_span() {
        let session = Session::new().no_stdout().no_file();
        let outer = session.operation("outer");
        let inner = session.operation("inner");

        {
            let _outer_span = outer.measure_thread().iterations(1);
            register_fake_allocation(10, 1);

            {
                let _inner_span = inner.measure_thread().iterations(1);
                register_fake_allocation(200, 1);
                register_fake_deallocation(200);
            }

            register_fake_deallocation(10);
        }

        // The inner span sees only its own 200, while the outer span sees the 210 that were
        // outstanding at once while the inner span ran.
        assert_eq!(inner.peak_outstanding_bytes(), Some(200));
        assert_eq!(outer.peak_outstanding_bytes(), Some(210));
    }

    #[test]
    fn abandoned_nested_span_does_not_suppress_the_enclosing_peak() {
        // The inner span is dropped during a panic and records nothing, but it must still
        // hand the watermark back or the outer span would lose its measurement.
        let session = Session::new().no_stdout().no_file();
        let outer = session.operation("outer");
        let inner = session.operation("inner");

        {
            let _outer_span = outer.measure_thread().iterations(1);

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _inner_span = inner.measure_thread().iterations(1);
                register_fake_allocation(400, 1);
                panic!("boom");
            }));
            assert!(result.is_err());

            register_fake_deallocation(400);
        }

        assert_eq!(inner.peak_outstanding_bytes(), None);
        assert_eq!(outer.peak_outstanding_bytes(), Some(400));
    }

    #[test]
    fn sequential_spans_report_their_own_peaks() {
        let session = Session::new().no_stdout().no_file();
        let operation = session.operation("test");

        {
            let _span = operation.measure_thread().iterations(1);
            register_fake_allocation(100, 1);
            register_fake_deallocation(100);
        }

        {
            let _span = operation.measure_thread().iterations(1);
            register_fake_allocation(700, 1);
            register_fake_deallocation(700);
        }

        {
            let _span = operation.measure_thread().iterations(1);
            register_fake_allocation(300, 1);
            register_fake_deallocation(300);
        }

        // The operation reports the highest moment across its spans, not their sum.
        assert_eq!(operation.peak_outstanding_bytes(), Some(700));
    }

    #[test]
    fn iterations_zero_is_accepted() {
        // A workload that could not run reports zero iterations; this records
        // rather than panicking, so the harness survives a failed benchmark.
        let session = Session::new().no_stdout().no_file();
        let operation = session.operation("test");

        drop(operation.measure_thread().iterations(0));

        assert_eq!(operation.total_iterations(), 0);
    }

    #[test]
    fn records_span_via_post_hoc_iterations() {
        let session = Session::new().no_stdout().no_file();
        let operation = session.operation("test");

        drop(operation.measure_thread().iterations(5));

        assert_eq!(operation.total_iterations(), 5);
    }

    #[test]
    fn records_span_via_iterations_guard() {
        let session = Session::new().no_stdout().no_file();
        let operation = session.operation("test");

        {
            let _span = operation.measure_thread().iterations(3);
        }

        assert_eq!(operation.total_iterations(), 3);
    }

    #[test]
    #[should_panic]
    fn dropping_span_without_iterations_panics() {
        let session = Session::new().no_stdout().no_file();
        let operation = session.operation("test");

        drop(operation.measure_thread());
    }

    #[test]
    fn panic_while_held_records_nothing() {
        let session = Session::new().no_stdout().no_file();
        let operation = session.operation("test");

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _span = operation.measure_thread().iterations(1);
            panic!("boom");
        }));

        assert!(result.is_err());
        assert_eq!(operation.total_iterations(), 0);
    }
}
