//! Thread-local allocation tracking span.

use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use crate::allocator::get_or_init_thread_counters;
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
    iterations: Option<u64>,

    _single_threaded: PhantomData<*const ()>,
}

impl ThreadSpan {
    pub(crate) fn new(operation: &Operation) -> Self {
        let counters = get_or_init_thread_counters();
        Self {
            metrics: operation.metrics(),
            start_bytes: counters.bytes(),
            start_count: counters.count(),
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
        let (bytes_delta, count_delta) = thread_deltas(self.start_bytes, self.start_count);
        let mut data = self.metrics.lock().expect(ERR_POISONED_LOCK);
        data.add_span(iterations, bytes_delta, count_delta);
    }
}

/// Computes the thread's allocation deltas since a span's start counters.
///
/// The whole-span deltas are returned undivided; per-iteration figures are derived
/// later by the shared span accumulator, which weights each span by its iteration
/// count.
fn thread_deltas(start_bytes: u64, start_count: u64) -> (u64, u64) {
    let counters = get_or_init_thread_counters();

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

    // Static assertions for thread safety.
    // The span should NOT be Send or Sync due to PhantomData<*const ()>.
    static_assertions::assert_not_impl_all!(ThreadSpan: Send);
    static_assertions::assert_not_impl_all!(ThreadSpan: Sync);

    // Static assertions for unwind safety.
    static_assertions::assert_impl_all!(ThreadSpan: UnwindSafe, RefUnwindSafe);

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
