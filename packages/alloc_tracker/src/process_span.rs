//! Process-wide allocation tracking span.

use std::cell::Cell;
use std::marker::PhantomData;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use crate::Operation;
use crate::allocator::TOTAL_BYTES_ALLOCATED;
use crate::constants::ERR_POISONED_LOCK;
use crate::session::OperationMetrics;

/// A tracked span of code that tracks process-wide allocations between creation and drop.
///
/// This span tracks allocations made by the entire process (all threads).
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
///     let _span = mean_calc.measure_process();
///     // Perform some operation that allocates memory
///     let _data = String::from("Hello, world!");
/// } // Memory delta is automatically tracked and recorded here
/// ```
#[derive(Debug)]
#[must_use = "Measurements are taken between creation and drop"]
pub struct ProcessSpan {
    metrics: Arc<Mutex<OperationMetrics>>,
    start_bytes: u64,
    iterations: u64,
    _not_sync: PhantomData<Cell<()>>,
}

impl ProcessSpan {
    pub(crate) fn new(operation: &Operation, iterations: u64) -> Self {
        assert!(iterations != 0);

        let start_bytes = TOTAL_BYTES_ALLOCATED.load(Ordering::Relaxed);

        Self {
            metrics: operation.metrics(),
            start_bytes,
            iterations,
            _not_sync: PhantomData,
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
    ///     let _span = operation.measure_process().iterations(1000);
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

    /// Calculates the allocation delta since this span was created.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // The != 1 fork is broadly applicable, so mutations fail. Intentional.
    fn to_delta(&self) -> u64 {
        let current_bytes = TOTAL_BYTES_ALLOCATED.load(Ordering::Relaxed);
        let total_delta = current_bytes
            .checked_sub(self.start_bytes)
            .expect("total bytes allocated could not possibly decrease");

        if self.iterations > 1 {
            // Divide total allocation by iterations to get per-iteration allocation
            total_delta
                .checked_div(self.iterations)
                .expect("guarded by if condition")
        } else {
            total_delta
        }
    }
}

impl Drop for ProcessSpan {
    fn drop(&mut self) {
        let delta = self.to_delta();

        let total_bytes = delta
            .checked_mul(self.iterations)
            .expect("bytes * iterations overflows u64 - this indicates an unrealistic scenario");

        let mut data = self.metrics.lock().expect(ERR_POISONED_LOCK);

        data.total_bytes_allocated = data
            .total_bytes_allocated
            .checked_add(total_bytes)
            .expect("total bytes allocated overflows u64 - this indicates an unrealistic scenario");

        data.total_iterations = data.total_iterations.checked_add(self.iterations).expect(
            "total iterations count overflows u64 - this indicates an unrealistic scenario",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Static assertions for thread safety
    static_assertions::assert_impl_all!(ProcessSpan: Send);
    static_assertions::assert_not_impl_any!(ProcessSpan: Sync);
    // ProcessSpan is Send but !Sync due to PhantomData<Cell<()>>
}
