use std::sync::LazyLock;

use crate::{
    pal::{Platform, PlatformFacade},
    MemoryRegionId, ProcessorId,
};

static CURRENT: LazyLock<HardwareInfo> =
    LazyLock::new(|| HardwareInfo::new(PlatformFacade::real()));

/// Reports basic information about the system hardware.
#[derive(Debug)]
pub struct HardwareInfo {
    max_processor_id: ProcessorId,
    max_memory_region_id: MemoryRegionId,
}

impl HardwareInfo {
    pub(crate) fn new(pal: PlatformFacade) -> Self {
        Self {
            max_processor_id: pal.max_processor_id(),
            max_memory_region_id: pal.max_memory_region_id(),
        }
    }

    /// The singleton instance of the type.
    pub fn current() -> &'static Self {
        &CURRENT
    }

    /// Gets the maximum (inclusive) processor ID of any processor that could possibly
    /// be present on the system (including processors that are not currently active).
    pub fn max_processor_id(&self) -> ProcessorId {
        self.max_processor_id
    }

    /// Gets the maximum (inclusive) memory region ID of any memory region that could possibly
    /// be present on the system (including memory regions that are not currently active).
    pub fn max_memory_region_id(&self) -> MemoryRegionId {
        self.max_memory_region_id
    }
}
