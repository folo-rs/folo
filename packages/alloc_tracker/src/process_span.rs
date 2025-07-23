//! Process-wide allocation tracking span.

use std::sync::atomic;

use crate::Operation;
use crate::allocator::TOTAL_BYTES_ALLOCATED;

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
/// let mut session = Session::new();
/// let mean_calc = session.operation("test");
/// {
///     let _span = mean_calc.iterations(1).measure_process();
///     // Perform some operation that allocates memory
///     let _data = String::from("Hello, world!");
/// } // Memory delta is automatically tracked and recorded here
/// ```
#[derive(Debug)]
#[must_use = "Measurements are taken between creation and drop"]
pub struct ProcessSpan<'a> {
    operation: &'a mut Operation,
    start_bytes: u64,
    iterations: u64,
}

impl<'a> ProcessSpan<'a> {
    pub(crate) fn new(operation: &'a mut Operation, iterations: u64) -> Self {
        assert!(iterations != 0);

        let start_bytes = TOTAL_BYTES_ALLOCATED.load(atomic::Ordering::Relaxed);

        Self {
            operation,
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

impl Drop for ProcessSpan<'_> {
    fn drop(&mut self) {
        let delta = self.to_delta();

        // Add the per-iteration delta for all iterations at once
        self.operation.add_iterations(delta, self.iterations);
    }
}
