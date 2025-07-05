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
///     let _span = mean_calc.measure_process();
///     // Perform some operation that allocates memory
///     let _data = String::from("Hello, world!");
/// } // Memory delta is automatically tracked and recorded here
/// ```
#[derive(Debug)]
pub struct ProcessSpan<'a> {
    operation: &'a mut Operation,
    start_bytes: u64,
}

impl<'a> ProcessSpan<'a> {
    pub(crate) fn new(operation: &'a mut Operation) -> Self {
        let start_bytes = TOTAL_BYTES_ALLOCATED.load(atomic::Ordering::Relaxed);

        Self {
            operation,
            start_bytes,
        }
    }

    /// Calculates the allocation delta since this span was created.
    #[must_use]
    fn to_delta(&self) -> u64 {
        let current_bytes = TOTAL_BYTES_ALLOCATED.load(atomic::Ordering::Relaxed);
        current_bytes
            .checked_sub(self.start_bytes)
            .expect("total bytes allocated could not possibly decrease")
    }
}

impl Drop for ProcessSpan<'_> {
    fn drop(&mut self) {
        self.operation.add(self.to_delta());
    }
}
