//! Thread-local allocation tracking span.

use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use crate::allocator::get_or_init_thread_counters;
use crate::buckets::BucketCounts;
use crate::{ERR_POISONED_LOCK, Operation, OperationMetrics};

/// A tracked span of code that tracks allocations on this thread between creation and drop.
///
/// This span tracks allocations made by the current thread only.
///
/// # Examples
///
/// ```
/// use alloc_tracker::{Allocator, Operation, Session};
///
/// #[global_allocator]
/// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
///
/// let session = Session::new();
/// let mean_calc = session.operation("test");
/// {
///     let _span = mean_calc.measure_thread();
///     // Perform some operation that allocates memory
///     let _data = String::from("Hello, world!");
/// } // Thread allocation is automatically tracked and recorded here
/// ```
#[derive(Debug)]
#[must_use = "Measurements are taken between creation and drop"]
pub struct ThreadSpan {
    metrics: Arc<Mutex<OperationMetrics>>,
    start_bytes: u64,
    start_count: u64,
    iterations: u64,
    start_buckets: Option<BucketCounts>,

    _single_threaded: PhantomData<*const ()>,
}

impl ThreadSpan {
    pub(crate) fn new(operation: &Operation, iterations: u64) -> Self {
        assert!(iterations != 0);

        let counters = get_or_init_thread_counters();
        let start_bytes = counters.bytes();
        let start_count = counters.count();
        let start_buckets = if operation.allocation_buckets_enabled() {
            Some(counters.bucket_counts())
        } else {
            None
        };

        Self {
            metrics: operation.metrics(),
            start_bytes,
            start_count,
            iterations,
            start_buckets,
            _single_threaded: PhantomData,
        }
    }

    /// Sets the number of iterations for this span.
    ///
    /// This allows you to specify how many iterations this span represents,
    /// which is used to calculate the mean allocation per iteration when the span is dropped.
    ///
    /// # Examples
    ///
    /// ```
    /// use alloc_tracker::{Allocator, Session};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
    ///
    /// let session = Session::new();
    /// let operation = session.operation("batch_work");
    /// {
    ///     let _span = operation.measure_thread().iterations(1000);
    ///     for _ in 0..1000 {
    ///         // Perform the same operation 1000 times
    ///         let _data = vec![42];
    ///     }
    /// } // Total allocation is measured once and divided by 1000
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

    /// Calculates the allocation deltas since this span was created.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // The != 1 fork is broadly applicable, so mutations fail. Intentional.
    fn to_deltas(&self) -> (u64, u64, Option<BucketCounts>) {
        let counters = get_or_init_thread_counters();
        let current_bytes = counters.bytes();
        let current_count = counters.count();

        let total_bytes_delta = current_bytes
            .checked_sub(self.start_bytes)
            .expect("thread bytes allocated could not possibly decrease");

        let total_count_delta = current_count
            .checked_sub(self.start_count)
            .expect("thread allocations count could not possibly decrease");

        let bucket_deltas = self
            .start_buckets
            .as_ref()
            .map(|start| start.delta_from(&counters.bucket_counts(), self.iterations));

        if self.iterations > 1 {
            // Divide total allocation by iterations to get per-iteration allocation
            let bytes_delta = total_bytes_delta
                .checked_div(self.iterations)
                .expect("guarded by if condition");
            let count_delta = total_count_delta
                .checked_div(self.iterations)
                .expect("guarded by if condition");
            (bytes_delta, count_delta, bucket_deltas)
        } else {
            (total_bytes_delta, total_count_delta, bucket_deltas)
        }
    }
}

impl Drop for ThreadSpan {
    fn drop(&mut self) {
        let (bytes_delta, count_delta, bucket_deltas) = self.to_deltas();
        let mut data = self.metrics.lock().expect(ERR_POISONED_LOCK);
        data.add_iterations(bytes_delta, count_delta, self.iterations);
        if let Some(deltas) = bucket_deltas {
            data.add_bucket_iterations(deltas, self.iterations);
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    // Static assertions for thread safety
    // ThreadSpan should NOT be Send or Sync due to PhantomData<*const ()>
    static_assertions::assert_not_impl_all!(ThreadSpan: Send);
    static_assertions::assert_not_impl_all!(ThreadSpan: Sync);
}
