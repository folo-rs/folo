use std::sync::LazyLock;

use crate::{
    MemoryRegionId, ProcessorId,
    pal::{Platform, PlatformFacade},
};

static CURRENT: LazyLock<HardwareInfo> =
    LazyLock::new(|| HardwareInfo::new(PlatformFacade::real()));

/// Reports non-changing information about the system hardware.
///
/// To inspect changing information, use [`HardwareTracker`][crate::HardwareTracker].
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
    #[inline]
    pub fn current() -> &'static Self {
        &CURRENT
    }

    /// Gets the maximum (inclusive) processor ID of any processor that could possibly
    /// be present on the system (including processors that are not currently active).
    #[inline]
    pub fn max_processor_id(&self) -> ProcessorId {
        self.max_processor_id
    }

    /// Gets the maximum (inclusive) memory region ID of any memory region that could possibly
    /// be present on the system (including memory regions that are not currently active).
    #[inline]
    pub fn max_memory_region_id(&self) -> MemoryRegionId {
        self.max_memory_region_id
    }

    /// Gets the maximum number of processors that could possibly be present on the system,
    /// including processors that are not currently active or available to this process.
    #[inline]
    pub fn max_processor_count(&self) -> usize {
        self.max_processor_id as usize + 1
    }

    /// Gets the maximum number of memory regions that could possibly be present on the system,
    /// including memory regions that are not currently active or available to this process.
    #[inline]
    pub fn max_memory_region_count(&self) -> usize {
        self.max_memory_region_id as usize + 1
    }
}

#[cfg(test)]
mod tests {
    use crate::pal::MockPlatform;

    use super::*;

    #[cfg(not(miri))] // Real platform is not supported under Miri.
    #[test]
    fn count_is_id_plus_one_real() {
        let info = HardwareInfo::current();

        assert_eq!(
            info.max_processor_count(),
            info.max_processor_id() as usize + 1
        );
        assert_eq!(
            info.max_memory_region_count(),
            info.max_memory_region_id() as usize + 1
        );
    }

    #[test]
    fn accurately_represents_platform() {
        let mut platform = MockPlatform::new();
        platform
            .expect_max_processor_id()
            .return_const(3 as ProcessorId);
        platform
            .expect_max_memory_region_id()
            .return_const(5 as MemoryRegionId);

        let platform = PlatformFacade::from_mock(platform);

        let info = HardwareInfo::new(platform);

        assert_eq!(info.max_processor_id(), 3);
        assert_eq!(info.max_memory_region_id(), 5);

        assert_eq!(
            info.max_processor_count(),
            info.max_processor_id() as usize + 1
        );
        assert_eq!(
            info.max_memory_region_count(),
            info.max_memory_region_id() as usize + 1
        );
    }
}
