//! Process-wide allocation tracking span.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic;

use crate::Operation;
use crate::allocator::TOTAL_BYTES_ALLOCATED;
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
///     let _span = mean_calc.iterations(1).measure_process();
///     // Perform some operation that allocates memory
///     let _data = String::from("Hello, world!");
/// } // Memory delta is automatically tracked and recorded here
/// ```
#[derive(Debug)]
#[must_use = "Measurements are taken between creation and drop"]
pub struct ProcessSpan {
    metrics: Rc<RefCell<OperationMetrics>>,
    start_bytes: u64,
    iterations: u64,
}

impl ProcessSpan {
    pub(crate) fn new(operation: &Operation, iterations: u64) -> Self {
        assert!(iterations != 0);

        let start_bytes = TOTAL_BYTES_ALLOCATED.load(atomic::Ordering::Relaxed);

        Self {
            metrics: operation.metrics(),
            start_bytes,
            iterations,
        }
    }

    /// Calculates the allocation delta since this span was created.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // The != 1 fork is broadly applicable, so mutations fail. Intentional.
    fn to_delta(&self) -> u64 {
        let current_bytes = TOTAL_BYTES_ALLOCATED.load(atomic::Ordering::Relaxed);
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

        let mut data = self.metrics.borrow_mut();

        data.total_bytes_allocated = data
            .total_bytes_allocated
            .checked_add(total_bytes)
            .expect("total bytes allocated overflows u64 - this indicates an unrealistic scenario");

        data.total_iterations = data.total_iterations.checked_add(self.iterations).expect(
            "total iterations count overflows u64 - this indicates an unrealistic scenario",
        );
    }
}
