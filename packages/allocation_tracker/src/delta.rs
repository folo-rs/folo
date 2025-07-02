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
