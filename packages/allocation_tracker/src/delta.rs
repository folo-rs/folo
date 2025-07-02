//! Memory delta tracking functionality.

use std::sync::atomic;

use crate::session::AllocationTrackingSession;
use crate::tracker::TRACKER_BYTES_ALLOCATED;

/// Tracks memory allocation changes over a specific time period.
///
/// This tracker captures the initial allocation count when created and can
/// calculate the delta (difference) in allocations when queried. It requires
/// an active allocation tracking session to function properly.
///
/// # Examples
///
/// ```
/// use allocation_tracker::{AllocationTrackingSession, MemoryDeltaTracker};
///
/// let session = AllocationTrackingSession::new().unwrap();
/// let tracker = MemoryDeltaTracker::new(&session);
/// let data = vec![1, 2, 3, 4, 5]; // This allocates memory
/// let delta = tracker.to_delta();
/// // delta will be >= the size of the vector allocation
/// ```
#[derive(Debug)]
pub struct MemoryDeltaTracker<'a> {
    initial_bytes_allocated: u64,
    _session: &'a AllocationTrackingSession,
}

impl<'a> MemoryDeltaTracker<'a> {
    /// Creates a new memory delta tracker, capturing the current allocation count.
    ///
    /// Requires an active allocation tracking session to ensure tracking is enabled.
    pub fn new(session: &'a AllocationTrackingSession) -> Self {
        let initial_bytes_allocated = TRACKER_BYTES_ALLOCATED.load(atomic::Ordering::Relaxed);

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
        let final_bytes_allocated = TRACKER_BYTES_ALLOCATED.load(atomic::Ordering::Relaxed);

        final_bytes_allocated
            .checked_sub(self.initial_bytes_allocated)
            .expect("total bytes allocated could not possibly decrease")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic;

    use super::*;
    use crate::tracker::TRACKER_BYTES_ALLOCATED;
    use crate::{AllocationTrackingSession, reset_allocation_counter};

    // Helper function to create a mock session for testing
    // Note: This won't actually enable allocation tracking since we're not using
    // a global allocator in unit tests, but it allows us to test the API structure
    fn create_test_session() -> AllocationTrackingSession {
        // For unit tests, we'll create a session that might fail but we'll ignore it
        // since these tests are focused on logic that doesn't require real allocation tracking
        AllocationTrackingSession::new().unwrap_or_else(|_| {
            // If session creation fails, we can still test the basic logic
            // This is a bit of a hack for unit testing
            AllocationTrackingSession { _private: () }
        })
    }

    #[test]
    fn memory_delta_tracker_new() {
        reset_allocation_counter();
        let session = create_test_session();
        let tracker = MemoryDeltaTracker::new(&session);
        assert_eq!(tracker.to_delta(), 0);
    }

    #[test]
    fn memory_delta_tracker_with_simulated_allocation() {
        reset_allocation_counter();
        let session = create_test_session();
        let tracker = MemoryDeltaTracker::new(&session);

        // Simulate some allocation by manually adding to the counter
        TRACKER_BYTES_ALLOCATED.fetch_add(150, atomic::Ordering::Relaxed);

        assert_eq!(tracker.to_delta(), 150);
    }

    #[test]
    fn memory_delta_tracker_multiple_trackers() {
        reset_allocation_counter();
        let session = create_test_session();
        let tracker1 = MemoryDeltaTracker::new(&session);

        // Simulate allocation
        TRACKER_BYTES_ALLOCATED.fetch_add(100, atomic::Ordering::Relaxed);

        let tracker2 = MemoryDeltaTracker::new(&session);

        // Simulate more allocation
        TRACKER_BYTES_ALLOCATED.fetch_add(50, atomic::Ordering::Relaxed);

        assert_eq!(tracker1.to_delta(), 150); // Total since tracker1 was created
        assert_eq!(tracker2.to_delta(), 50); // Only since tracker2 was created
    }
}
