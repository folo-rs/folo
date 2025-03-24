use crate::{HardwareTracker, MemoryRegionId, ProcessorId};

/// Returns the ID of the processor that the current thread is executing on.
///
/// Convenience function to access the singleton hardware tracker instance
/// and perform the relevant query on it.
#[cfg_attr(test, mutants::skip)] // Impractical to test; only lower levels covered.
pub fn current_processor_id() -> ProcessorId {
    HardwareTracker::with(|tracker| tracker.current_processor_id())
}

/// Returns the ID of the memory region of the processor that the current thread is executing on.
///
/// Convenience function to access the singleton hardware tracker instance
/// and perform the relevant query on it.
#[cfg_attr(test, mutants::skip)] // Impractical to test; only lower levels covered.
pub fn current_memory_region_id() -> MemoryRegionId {
    HardwareTracker::with(|tracker| tracker.current_memory_region_id())
}

#[cfg(test)]
mod tests {
    use crate::HardwareInfo;

    use super::*;

    #[test]
    fn smoke_test() {
        // This is working against a real environment, so the most we can really do here
        // is to ensure it does not panic. Any IDs are valid, really.
        let processor_id = current_processor_id();
        let memory_region_id = current_memory_region_id();

        // We can at least check that the IDs are not above the maximums.
        assert!(processor_id < HardwareInfo::current().max_processor_id());
        assert!(memory_region_id < HardwareInfo::current().max_memory_region_id());
    }
}
