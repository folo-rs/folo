//! Thread-local allocation tracking measurement and span.

use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use crate::allocator::get_or_init_thread_counters;
use crate::{ERR_POISONED_LOCK, Operation, OperationMetrics};

/// An in-progress measurement of this thread's allocations, awaiting an explicit
/// iteration count.
///
/// Returned by [`Operation::measure_thread`](crate::Operation::measure_thread).
/// It captures the thread's allocation counters at creation but records nothing on
/// its own — the caller must state how many iterations the measured work covers,
/// either up front with [`iterations`](Self::iterations) (an RAII guard that
/// records when its scope ends) or afterwards with [`complete`](Self::complete).
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
/// use alloc_tracker::{Allocator, Session};
/// use criterion::Criterion;
///
/// #[global_allocator]
/// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
///
/// # fn main() {
/// let session = Session::new();
/// let operation = session.operation("allocate_buffer");
/// let mut criterion = Criterion::default();
/// criterion.bench_function("allocate_buffer", |b| {
///     b.iter_custom(|iters| {
///         let start = Instant::now();
///         let _span = operation.measure_thread().iterations(iters);
///         for _ in 0..iters {
///             black_box(vec![1_u8; 64]);
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
    start_bytes: u64,
    start_count: u64,

    _single_threaded: PhantomData<*const ()>,
}

impl ThreadMeasurement {
    pub(crate) fn new(operation: &Operation) -> Self {
        let counters = get_or_init_thread_counters();
        Self {
            metrics: operation.metrics(),
            start_bytes: counters.bytes(),
            start_count: counters.count(),
            _single_threaded: PhantomData,
        }
    }

    /// Binds this measurement to an iteration count known in advance, returning an
    /// RAII guard that records the thread's allocation delta when it is dropped.
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
            start_bytes: self.start_bytes,
            start_count: self.start_count,
            iterations,
            _single_threaded: PhantomData,
        }
    }

    /// Records the thread's allocation delta now, attributing it to `iterations`
    /// iterations.
    ///
    /// Use this when the iteration count is only known after the measured work has
    /// run — for example a loop that runs until some budget is exhausted.
    ///
    /// # Examples
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
    /// // The iteration count is only known once the work has finished.
    /// let measurement = operation.measure_thread();
    /// let mut processed = 0_u64;
    /// for item in 0..5 {
    ///     let _data = vec![item; 8]; // allocate while draining
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
        let (bytes_delta, count_delta) = thread_deltas(self.start_bytes, self.start_count);
        let mut data = self.metrics.lock().expect(ERR_POISONED_LOCK);
        data.add_span(iterations, bytes_delta, count_delta);
    }
}

/// An active thread-allocation measurement bound to a known iteration count.
///
/// Created by [`ThreadMeasurement::iterations`]. It records the thread's
/// allocation delta when dropped, so the measured work should live inside the
/// span's scope.
#[derive(Debug)]
#[must_use = "the measurement is recorded when the span is dropped, so it must be held for the measured work"]
pub struct ThreadSpan {
    metrics: Arc<Mutex<OperationMetrics>>,
    start_bytes: u64,
    start_count: u64,
    iterations: u64,

    _single_threaded: PhantomData<*const ()>,
}

impl Drop for ThreadSpan {
    fn drop(&mut self) {
        let (bytes_delta, count_delta) = thread_deltas(self.start_bytes, self.start_count);
        let mut data = self.metrics.lock().expect(ERR_POISONED_LOCK);
        data.add_span(self.iterations, bytes_delta, count_delta);
    }
}

/// Computes the thread's allocation deltas since a measurement's start counters.
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
    // Both types should NOT be Send or Sync due to PhantomData<*const ()>.
    static_assertions::assert_not_impl_all!(ThreadMeasurement: Send);
    static_assertions::assert_not_impl_all!(ThreadMeasurement: Sync);
    static_assertions::assert_not_impl_all!(ThreadSpan: Send);
    static_assertions::assert_not_impl_all!(ThreadSpan: Sync);

    // Static assertions for unwind safety.
    static_assertions::assert_impl_all!(ThreadMeasurement: UnwindSafe, RefUnwindSafe);
    static_assertions::assert_impl_all!(ThreadSpan: UnwindSafe, RefUnwindSafe);

    #[test]
    fn records_span_via_complete() {
        let session = Session::new().no_stdout().no_file();
        let operation = session.operation("test");

        operation.measure_thread().complete(5);

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
    fn dropping_measurement_without_terminal_records_nothing() {
        let session = Session::new().no_stdout().no_file();
        let operation = session.operation("test");

        drop(operation.measure_thread());

        assert_eq!(operation.total_iterations(), 0);
    }
}
