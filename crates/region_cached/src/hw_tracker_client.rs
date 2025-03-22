#[cfg(test)]
use std::sync::Arc;

use many_cpus::{HardwareTracker, MemoryRegionId};

#[cfg_attr(test, mockall::automock)]
pub(crate) trait HardwareTrackerClient {
    fn current_memory_region_id(&self) -> MemoryRegionId;
    fn is_thread_memory_region_pinned(&self) -> bool;
}

#[derive(Debug)]
pub(crate) struct HardwareTrackerClientImpl;

impl HardwareTrackerClient for HardwareTrackerClientImpl {
    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn current_memory_region_id(&self) -> MemoryRegionId {
        HardwareTracker::with(|tracker| tracker.current_memory_region_id())
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn is_thread_memory_region_pinned(&self) -> bool {
        HardwareTracker::with(|tracker| tracker.is_thread_memory_region_pinned())
    }
}

#[derive(Clone, Debug)]
pub(crate) enum HardwareTrackerClientFacade {
    Real(&'static HardwareTrackerClientImpl),

    #[cfg(test)]
    Mock(Arc<MockHardwareTrackerClient>),
}

impl HardwareTrackerClientFacade {
    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    pub(crate) const fn real() -> Self {
        Self::Real(&HardwareTrackerClientImpl)
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    #[cfg(test)]
    pub(crate) fn from_mock(mock: MockHardwareTrackerClient) -> Self {
        Self::Mock(Arc::new(mock))
    }
}

impl HardwareTrackerClient for HardwareTrackerClientFacade {
    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn current_memory_region_id(&self) -> MemoryRegionId {
        match self {
            HardwareTrackerClientFacade::Real(real) => real.current_memory_region_id(),
            #[cfg(test)]
            HardwareTrackerClientFacade::Mock(mock) => mock.current_memory_region_id(),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn is_thread_memory_region_pinned(&self) -> bool {
        match self {
            HardwareTrackerClientFacade::Real(real) => real.is_thread_memory_region_pinned(),
            #[cfg(test)]
            HardwareTrackerClientFacade::Mock(mock) => mock.is_thread_memory_region_pinned(),
        }
    }
}
