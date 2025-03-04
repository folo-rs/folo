use folo_hw::{HardwareTracker, MemoryRegionId};

#[cfg_attr(test, mockall::automock)]
pub(crate) trait HardwareTrackerClient {
    fn current_memory_region_id(&self) -> MemoryRegionId;
}

#[derive(Debug)]
pub(crate) struct HardwareTrackerClientImpl;

impl HardwareTrackerClient for HardwareTrackerClientImpl {
    fn current_memory_region_id(&self) -> MemoryRegionId {
        HardwareTracker::with_current(|tracker| tracker.current_memory_region_id())
    }
}

#[derive(Debug)]
pub(crate) enum HardwareTrackerClientFacade {
    Real(&'static HardwareTrackerClientImpl),

    #[cfg(test)]
    Mock(MockHardwareTrackerClient),
}

impl HardwareTrackerClientFacade {
    pub(crate) const fn real() -> Self {
        Self::Real(&HardwareTrackerClientImpl)
    }

    #[cfg(test)]
    pub(crate) fn from_mock(mock: MockHardwareTrackerClient) -> Self {
        Self::Mock(mock)
    }
}

impl HardwareTrackerClient for HardwareTrackerClientFacade {
    fn current_memory_region_id(&self) -> MemoryRegionId {
        match self {
            HardwareTrackerClientFacade::Real(real) => real.current_memory_region_id(),
            #[cfg(test)]
            HardwareTrackerClientFacade::Mock(mock) => mock.current_memory_region_id(),
        }
    }
}
