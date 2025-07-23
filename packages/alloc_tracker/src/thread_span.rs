//! Thread-local allocation tracking span.

use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

use crate::constants::ERR_POISONED_LOCK;
use crate::Operation;
use crate::allocator::THREAD_BYTES_ALLOCATED;
use crate::session::OperationMetrics;

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
///     let _span = mean_calc.iterations(1).measure_thread();
///     // Perform some operation that allocates memory
///     let _data = String::from("Hello, world!");
/// } // Thread allocation is automatically tracked and recorded here
/// ```
#[derive(Debug)]
#[must_use = "Measurements are taken between creation and drop"]
pub struct ThreadSpan {
    metrics: Arc<Mutex<OperationMetrics>>,
    start_bytes: u64,
    iterations: u64,

    _single_threaded: PhantomData<*const ()>,
}

impl ThreadSpan {
    pub(crate) fn new(operation: &Operation, iterations: u64) -> Self {
        assert!(iterations != 0);

        let start_bytes = THREAD_BYTES_ALLOCATED.with(std::cell::Cell::get);

        Self {
            metrics: operation.metrics(),
            start_bytes,
            iterations,
            _single_threaded: PhantomData,
        }
    }

    /// Calculates the allocation delta since this span was created.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // The != 1 fork is broadly applicable, so mutations fail. Intentional.
    fn to_delta(&self) -> u64 {
        let current_bytes = THREAD_BYTES_ALLOCATED.with(std::cell::Cell::get);
        let total_delta = current_bytes
            .checked_sub(self.start_bytes)
            .expect("thread bytes allocated could not possibly decrease");

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

impl Drop for ThreadSpan {
    fn drop(&mut self) {
        let delta = self.to_delta();

        let total_bytes = delta
            .checked_mul(self.iterations)
            .expect("bytes * iterations overflows u64 - this indicates an unrealistic scenario");

        let mut data = self.metrics.lock()
            .expect(ERR_POISONED_LOCK);

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
    // ThreadSpan should NOT be Send or Sync due to PhantomData<*const ()>
    static_assertions::assert_not_impl_all!(ThreadSpan: Send);
    static_assertions::assert_not_impl_all!(ThreadSpan: Sync);
}
