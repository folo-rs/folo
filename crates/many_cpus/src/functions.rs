use crate::{HardwareTracker, MemoryRegionId, ProcessorId};

/// Returns the ID of the processor that the current thread is executing on.
/// 
/// Convenience function to access the singleton hardware tracker instance
/// and perform the relevant query on it.
pub fn current_processor_id() -> ProcessorId {
    HardwareTracker::with(|tracker| tracker.current_processor_id())
}

/// Returns the ID of the memory region of the processor that the current thread is executing on.
/// 
/// Convenience function to access the singleton hardware tracker instance
/// and perform the relevant query on it.
pub fn current_memory_region_id() -> MemoryRegionId {
    HardwareTracker::with(|tracker| tracker.current_memory_region_id())
}
