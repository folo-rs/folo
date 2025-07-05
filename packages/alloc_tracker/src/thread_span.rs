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
///     let _span = mean_calc.measure_thread();
///     // Perform some operation that allocates memory
///     let _data = String::from("Hello, world!");
/// } // Thread allocation is automatically tracked and recorded here
/// ```
#[derive(Debug)]
pub struct ThreadSpan<'a> {
    operation: &'a mut Operation,
    start_bytes: u64,

    _single_threaded: PhantomData<*const ()>,
}

impl<'a> ThreadSpan<'a> {
    pub(crate) fn new(operation: &'a mut Operation) -> Self {
        let start_bytes = THREAD_BYTES_ALLOCATED.with(std::cell::Cell::get);

        Self {
            operation,
            start_bytes,
            _single_threaded: PhantomData,
        }
    }

    /// Calculates the allocation delta since this span was created.
    #[must_use]
    fn to_delta(&self) -> u64 {
        let current_bytes = THREAD_BYTES_ALLOCATED.with(std::cell::Cell::get);
        current_bytes
            .checked_sub(self.start_bytes)
            .expect("thread bytes allocated could not possibly decrease")
    }
}

impl Drop for ThreadSpan<'_> {
    fn drop(&mut self) {
        self.operation.add(self.to_delta());
    }
}
