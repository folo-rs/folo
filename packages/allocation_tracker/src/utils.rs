//! Utility functions for allocation tracking.

use std::sync::atomic;

use crate::tracker::TRACKER_BYTES_ALLOCATED;

/// Resets the internal allocation counter to zero.
///
/// This function is primarily intended for testing purposes to ensure a clean
/// state between test runs. It should not be used in production code as it can
/// interfere with concurrent allocation tracking.
///
/// # Examples
///
/// ```
/// use allocation_tracker::{AllocationTrackingSession, MemoryDeltaTracker, reset_allocation_counter};
///
/// let session = AllocationTrackingSession::new();
/// reset_allocation_counter();
/// let tracker = MemoryDeltaTracker::new(&session);
/// let data = vec![1, 2, 3];
/// let delta = tracker.to_delta();
/// // delta now reflects only the allocation since reset
/// ```
pub fn reset_allocation_counter() {
    TRACKER_BYTES_ALLOCATED.store(0, atomic::Ordering::Relaxed);
}

/// Returns the current total bytes allocated since the counter was last reset.
///
/// This function provides direct access to the internal allocation counter
/// and is primarily intended for testing and diagnostic purposes.
///
/// # Examples
///
/// ```
/// use allocation_tracker::current_allocation_count;
///
/// let initial = current_allocation_count();
/// let data = vec![1, 2, 3, 4, 5];
/// let after = current_allocation_count();
/// assert!(after >= initial);
/// ```
pub fn current_allocation_count() -> u64 {
    TRACKER_BYTES_ALLOCATED.load(atomic::Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic;

    use super::*;
    use crate::tracker::TRACKER_BYTES_ALLOCATED;

    #[test]
    fn reset_and_current_allocation_count() {
        // Set some value
        TRACKER_BYTES_ALLOCATED.store(500, atomic::Ordering::Relaxed);
        assert_eq!(current_allocation_count(), 500);

        // Reset to zero
        reset_allocation_counter();
        assert_eq!(current_allocation_count(), 0);
    }

    /// This test ensures our internal access to `TRACKER_BYTES_ALLOCATED` works correctly.
    #[test]
    fn internal_counter_access() {
        reset_allocation_counter();
        assert_eq!(current_allocation_count(), 0);

        TRACKER_BYTES_ALLOCATED.fetch_add(42, atomic::Ordering::Relaxed);
        assert_eq!(current_allocation_count(), 42);

        reset_allocation_counter();
        assert_eq!(current_allocation_count(), 0);
    }
}
