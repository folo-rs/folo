//! Session management for allocation tracking.

use std::sync::atomic::AtomicBool;
use std::sync::{OnceLock, atomic};

use tracking_allocator::AllocationRegistry;

use crate::tracker::MemoryTracker;

/// Manages allocation tracking session state.
///
/// This type ensures that allocation tracking is properly enabled and disabled,
/// and prevents multiple concurrent tracking sessions which would interfere with
/// each other.
///
/// # Examples
///
/// ```rust
/// use allocation_tracker::{AllocationTrackingSession, MemoryDeltaTracker};
///
/// let session = AllocationTrackingSession::new();
/// let tracker = MemoryDeltaTracker::new(&session);
/// let data = vec![1, 2, 3, 4, 5];
/// let delta = tracker.to_delta();
/// // Session automatically disables tracking when dropped
/// ```
#[derive(Debug)]
pub struct AllocationTrackingSession {
    pub(crate) _private: (),
}

static TRACKING_SESSION_ACTIVE: AtomicBool = AtomicBool::new(false);
static TRACKER_INITIALIZED: OnceLock<()> = OnceLock::new();

impl AllocationTrackingSession {
    /// Creates a new allocation tracking session.
    ///
    /// This will automatically set up the global tracker (on first use) and enable
    /// allocation tracking. Only one session can be active at a time.
    ///
    /// # Panics
    ///
    /// Panics if another allocation tracking session is already active.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use allocation_tracker::AllocationTrackingSession;
    ///
    /// let session = AllocationTrackingSession::new();
    /// // Allocation tracking is now enabled
    /// // Session will disable tracking when dropped
    /// ```
    #[allow(clippy::new_without_default, reason = "Default implementation would be inappropriate as new() can panic")]
    pub fn new() -> Self {
        // Initialize the tracker on first use
        TRACKER_INITIALIZED.get_or_init(|| {
            AllocationRegistry::set_global_tracker(MemoryTracker::new())
                .expect("failed to set global allocation tracker");
        });

        // Try to acquire the session lock
        if TRACKING_SESSION_ACTIVE
            .compare_exchange(
                false,
                true,
                atomic::Ordering::Acquire,
                atomic::Ordering::Relaxed,
            )
            .is_err()
        {
            panic!("allocation tracking session is already active");
        }

        AllocationRegistry::enable_tracking();

        Self { _private: () }
    }
}

impl Drop for AllocationTrackingSession {
    fn drop(&mut self) {
        AllocationRegistry::disable_tracking();
        TRACKING_SESSION_ACTIVE.store(false, atomic::Ordering::Release);
    }
}


