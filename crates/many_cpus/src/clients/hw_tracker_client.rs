use crate::{CURRENT_TRACKER, MemoryRegionId, ProcessorId};

#[cfg_attr(test, mockall::automock)]
pub(crate) trait HardwareTrackerClient {
    fn update_pin_status(
        &self,
        processor_id: Option<ProcessorId>,
        memory_region_id: Option<MemoryRegionId>,
    );
}

#[derive(Debug)]
pub(crate) struct HardwareTrackerClientImpl;

impl HardwareTrackerClient for HardwareTrackerClientImpl {
    #[cfg_attr(test, mutants::skip)] // Trivial fn, tested on lower levels - skip mutating.
    fn update_pin_status(
        &self,
        processor_id: Option<ProcessorId>,
        memory_region_id: Option<MemoryRegionId>,
    ) {
        CURRENT_TRACKER.with_borrow_mut(|tracker| {
            tracker.update_pin_status(processor_id, memory_region_id);
        });
    }
}
