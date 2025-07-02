//! Core memory tracking functionality.

use std::sync::atomic;
use std::sync::atomic::AtomicU64;

use tracking_allocator::AllocationTracker;

// The tracking allocator works with static data all over the place, so this is how it be.
pub(crate) static TRACKER_BYTES_ALLOCATED: AtomicU64 = AtomicU64::new(0);

/// A memory allocation tracker that measures allocation activity.
///
/// This tracker only counts allocations, not deallocations, as it is designed
/// to measure the memory footprint of operations rather than the net memory usage.
#[derive(Debug)]
#[non_exhaustive]
pub(crate) struct MemoryTracker {
    _private: (),
}

impl MemoryTracker {
    /// Creates a new memory tracker instance.
    #[must_use]
    pub(crate) const fn new() -> Self {
        Self { _private: () }
    }
}

impl Default for MemoryTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl AllocationTracker for MemoryTracker {
    fn allocated(
        &self,
        _addr: usize,
        object_size: usize,
        _wrapped_size: usize,
        _group_id: tracking_allocator::AllocationGroupId,
    ) {
        TRACKER_BYTES_ALLOCATED.fetch_add(object_size as u64, atomic::Ordering::Relaxed);
    }

    fn deallocated(
        &self,
        _addr: usize,
        _object_size: usize,
        _wrapped_size: usize,
        _source_group_id: tracking_allocator::AllocationGroupId,
        _current_group_id: tracking_allocator::AllocationGroupId,
    ) {
        // We intentionally do not track deallocations as we want to measure
        // total allocation footprint, not net memory usage.
    }
}
