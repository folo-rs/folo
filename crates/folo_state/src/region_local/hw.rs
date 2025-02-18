use folo_hw::{HardwareTracker, MemoryRegionId};

/// Abstraction over the hardware metadata that region-local storage requires.
/// This allows us to substitute mock hardware metadata in tests.
#[cfg_attr(test, mockall::automock)]
pub(crate) trait Hardware {
    fn current_memory_region_id(&self) -> MemoryRegionId;
}

#[derive(Debug)]
pub(crate) struct HardwareImpl;

impl Hardware for HardwareImpl {
    fn current_memory_region_id(&self) -> MemoryRegionId {
        HardwareTracker::with_current(|tracker| tracker.current_memory_region_id())
    }
}

#[derive(Debug)]
pub(crate) enum HardwareFacade {
    Real(&'static HardwareImpl),

    #[cfg(test)]
    Mock(MockHardware),
}

impl HardwareFacade {
    pub(crate) const fn real() -> Self {
        Self::Real(&HardwareImpl)
    }

    #[cfg(test)]
    pub(crate) fn from_mock(mock: MockHardware) -> Self {
        Self::Mock(mock)
    }
}

impl Hardware for HardwareFacade {
    fn current_memory_region_id(&self) -> MemoryRegionId {
        match self {
            HardwareFacade::Real(real) => real.current_memory_region_id(),
            #[cfg(test)]
            HardwareFacade::Mock(mock) => mock.current_memory_region_id(),
        }
    }
}
