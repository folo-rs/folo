//! Memory delta tracking functionality.

use std::sync::atomic;

use crate::session::Session;
use crate::tracker::TOTAL_BYTES_ALLOCATED;

/// Tracks memory allocation changes over a specific time period.
///
/// This tracker captures the initial allocation count when created and can
/// calculate the delta (difference) in allocations when queried. It requires
/// an active allocation tracking session to function properly.
///
/// # Examples
///
/// ```
/// use alloc_tracker::{Allocator, Session, Span};
///
/// #[global_allocator]
/// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
///
/// let session = Session::new();
/// let span = Span::new(&session);
/// let data = vec![1, 2, 3, 4, 5]; // This allocates memory
/// let delta = span.to_delta();
/// // delta will be >= the size of the vector allocation
/// ```
#[derive(Debug)]
pub struct Span<'a> {
    initial_bytes_allocated: u64,
    _session: &'a Session,
}

impl<'a> Span<'a> {
    /// Creates a new memory delta tracker, capturing the current allocation count.
    ///
    /// Requires an active allocation tracking session to ensure tracking is enabled.
    pub fn new(session: &'a Session) -> Self {
        let initial_bytes_allocated = TOTAL_BYTES_ALLOCATED.load(atomic::Ordering::Relaxed);

        Self {
            initial_bytes_allocated,
            _session: session,
        }
    }

    /// Calculates the memory allocation delta since this tracker was created.
    ///
    /// # Panics
    ///
    /// Panics if the total bytes allocated has somehow decreased since the tracker
    /// was created, which should never happen in normal operation.
    pub fn to_delta(&self) -> u64 {
        let final_bytes_allocated = TOTAL_BYTES_ALLOCATED.load(atomic::Ordering::Relaxed);

        final_bytes_allocated
            .checked_sub(self.initial_bytes_allocated)
            .expect("total bytes allocated could not possibly decrease")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic;

    use super::*;
    use crate::Session;
    use crate::tracker::TOTAL_BYTES_ALLOCATED;

    // Helper function to create a mock session for testing
    // Note: This won't actually enable allocation tracking since we're not using
    // a global allocator in unit tests, but it allows us to test the API structure
    fn create_test_session() -> Session {
        // This function simplifies test setup by providing a session
        // We can now directly call new() since it's infallible
        Session::new()
    }

    #[test]
    fn span_new() {
        let session = create_test_session();
        let span = Span::new(&session);
        assert_eq!(span.to_delta(), 0);
    }

    #[test]
    fn span_with_simulated_allocation() {
        let session = create_test_session();
        let span = Span::new(&session);

        // Simulate some allocation by manually adding to the counter
        TOTAL_BYTES_ALLOCATED.fetch_add(150, atomic::Ordering::Relaxed);

        assert_eq!(span.to_delta(), 150);
    }

    #[test]
    fn span_multiple_spans() {
        let session = create_test_session();
        let span1 = Span::new(&session);

        // Simulate allocation
        TOTAL_BYTES_ALLOCATED.fetch_add(100, atomic::Ordering::Relaxed);

        let span2 = Span::new(&session);

        // Simulate more allocation
        TOTAL_BYTES_ALLOCATED.fetch_add(50, atomic::Ordering::Relaxed);

        assert_eq!(span1.to_delta(), 150); // Total since span1 was created
        assert_eq!(span2.to_delta(), 50); // Only since span2 was created
    }
}
