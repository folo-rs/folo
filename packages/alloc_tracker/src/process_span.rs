//! Process-wide allocation tracking measurement and span.

use std::cell::Cell;
use std::marker::PhantomData;
use std::panic::RefUnwindSafe;
use std::sync::{Arc, Mutex};

use crate::allocator::{AllocationTotals, allocation_totals};
use crate::{ERR_POISONED_LOCK, Operation, OperationMetrics};

/// An in-progress measurement of process-wide allocations, awaiting an explicit
/// iteration count.
///
/// Returned by [`Operation::measure_process`](crate::Operation::measure_process).
/// It captures the process's allocation counters at creation but records nothing
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
///         let _span = operation.measure_process().iterations(iters);
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
pub struct ProcessMeasurement {
    metrics: Arc<Mutex<OperationMetrics>>,
    start_bytes: u64,
    start_count: u64,
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
        let AllocationTotals {
            bytes: start_bytes,
            count: start_count,
        } = allocation_totals();

        Self {
            metrics: operation.metrics(),
            start_bytes,
            start_count,
            _not_sync: PhantomData,
        }
    }

    /// Binds this measurement to an iteration count known in advance, returning an
    /// RAII guard that records the process's allocation delta when it is dropped.
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
            start_bytes: self.start_bytes,
            start_count: self.start_count,
            iterations,
            _not_sync: PhantomData,
        }
    }

    /// Records the process's allocation delta now, attributing it to `iterations`
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
    /// let measurement = operation.measure_process();
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
        let (bytes_delta, count_delta) = process_deltas(self.start_bytes, self.start_count);
        let mut data = self.metrics.lock().expect(ERR_POISONED_LOCK);
        data.add_span(iterations, bytes_delta, count_delta);
    }
}

/// An active process-wide allocation measurement bound to a known iteration count.
///
/// Created by [`ProcessMeasurement::iterations`]. It records the process's
/// allocation delta when dropped, so the measured work should live inside the
/// span's scope.
#[derive(Debug)]
#[must_use = "the measurement is recorded when the span is dropped, so it must be held for the measured work"]
pub struct ProcessSpan {
    metrics: Arc<Mutex<OperationMetrics>>,
    start_bytes: u64,
    start_count: u64,
    iterations: u64,
    _not_sync: PhantomData<Cell<()>>,
}

// The Cell<()> marker is zero-sized with no actual mutable state, so there is nothing to
// observe in an inconsistent state during unwind.
impl RefUnwindSafe for ProcessSpan {}

impl Drop for ProcessSpan {
    fn drop(&mut self) {
        let (bytes_delta, count_delta) = process_deltas(self.start_bytes, self.start_count);
        let mut data = self.metrics.lock().expect(ERR_POISONED_LOCK);
        data.add_span(self.iterations, bytes_delta, count_delta);
    }
}

/// Computes the process-wide allocation deltas since a measurement's start
/// counters.
///
/// The whole-span deltas are returned undivided; per-iteration figures are derived
/// later by the shared span accumulator, which weights each span by its iteration
/// count.
fn process_deltas(start_bytes: u64, start_count: u64) -> (u64, u64) {
    let AllocationTotals {
        bytes: current_bytes,
        count: current_count,
    } = allocation_totals();

    let bytes_delta = current_bytes
        .checked_sub(start_bytes)
        .expect("total bytes allocated could not possibly decrease");
    let count_delta = current_count
        .checked_sub(start_count)
        .expect("total allocations count could not possibly decrease");

    (bytes_delta, count_delta)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use super::*;
    use crate::Session;

    // Static assertions for thread safety.
    // Both types are Send but !Sync due to PhantomData<Cell<()>>.
    static_assertions::assert_impl_all!(ProcessMeasurement: Send, UnwindSafe, RefUnwindSafe);
    static_assertions::assert_not_impl_any!(ProcessMeasurement: Sync);
    static_assertions::assert_impl_all!(ProcessSpan: Send, UnwindSafe, RefUnwindSafe);
    static_assertions::assert_not_impl_any!(ProcessSpan: Sync);

    #[test]
    fn records_span_via_complete() {
        let session = Session::new().no_stdout().no_file();
        let operation = session.operation("test");

        operation.measure_process().complete(5);

        assert_eq!(operation.total_iterations(), 5);
    }

    #[test]
    fn records_span_via_iterations_guard() {
        let session = Session::new().no_stdout().no_file();
        let operation = session.operation("test");

        {
            let _span = operation.measure_process().iterations(3);
        }

        assert_eq!(operation.total_iterations(), 3);
    }

    #[test]
    fn dropping_measurement_without_terminal_records_nothing() {
        let session = Session::new().no_stdout().no_file();
        let operation = session.operation("test");

        drop(operation.measure_process());

        assert_eq!(operation.total_iterations(), 0);
    }
}
