//! Core memory tracking functionality.

use std::sync::atomic;
use std::sync::atomic::AtomicU64;

use tracking_allocator::AllocationTracker;

// The tracking allocator works with static data all over the place, so this is how it be.
// This is global state - we could theoretically optimize via thread-local counter but that
// might only matter in extremely allocation-heavy scenarios which are not a priority (yet?).
pub(crate) static TOTAL_BYTES_ALLOCATED: AtomicU64 = AtomicU64::new(0);

/// This type exists to wire the `impl AllocationTracker` (below) into our global state (above).
/// The first `AllocationTrackingSession` created will set this as the global tracker.
#[derive(Debug)]
pub(crate) struct MemoryTracker;

impl AllocationTracker for MemoryTracker {
    fn allocated(
        &self,
        _addr: usize,
        object_size: usize,
        _wrapped_size: usize,
        _group_id: tracking_allocator::AllocationGroupId,
    ) {
        TOTAL_BYTES_ALLOCATED.fetch_add(object_size as u64, atomic::Ordering::Relaxed);
    }

    fn deallocated(
        &self,
        _addr: usize,
        _object_size: usize,
        _wrapped_size: usize,
        _source_group_id: tracking_allocator::AllocationGroupId,
        _current_group_id: tracking_allocator::AllocationGroupId,
    ) {
        // We do not track deallocations (at least not yet).
    }
}
