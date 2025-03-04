use crate::{HardwareTracker, MemoryRegionId, ProcessorId};

pub fn current_processor_id() -> ProcessorId {
    HardwareTracker::with_current(|tracker| tracker.current_processor_id())
}

pub fn current_memory_region_id() -> MemoryRegionId {
    HardwareTracker::with_current(|tracker| tracker.current_memory_region_id())
}
