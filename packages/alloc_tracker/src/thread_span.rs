//! Thread-local allocation tracking span.

use std::marker::PhantomData;

use crate::Operation;
use crate::allocator::THREAD_BYTES_ALLOCATED;

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
/// let mut session = Session::new();
/// let mean_calc = session.operation("test");
/// {
///     let _span = mean_calc.iterations(1).measure_thread();
///     // Perform some operation that allocates memory
///     let _data = String::from("Hello, world!");
/// } // Thread allocation is automatically tracked and recorded here
/// ```
#[derive(Debug)]
#[must_use = "Measurements are taken between creation and drop"]
pub struct ThreadSpan<'a> {
    operation: &'a mut Operation,
    start_bytes: u64,
    iterations: u64,

    _single_threaded: PhantomData<*const ()>,
}

impl<'a> ThreadSpan<'a> {
    pub(crate) fn new(operation: &'a mut Operation, iterations: u64) -> Self {
        assert!(iterations != 0);

        let start_bytes = THREAD_BYTES_ALLOCATED.with(std::cell::Cell::get);

        Self {
            operation,
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

impl Drop for ThreadSpan<'_> {
    fn drop(&mut self) {
        let delta = self.to_delta();

        // Add the per-iteration delta for all iterations at once
        self.operation.add_iterations(delta, self.iterations);
    }
}
