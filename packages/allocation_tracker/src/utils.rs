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
/// use allocation_tracker::{reset_allocation_counter, MemoryDeltaTracker};
///
/// reset_allocation_counter();
/// let tracker = MemoryDeltaTracker::new();
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
