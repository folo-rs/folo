#[cfg(test)]
use std::sync::Arc;

use many_cpus::MemoryRegionId;

#[cfg(test)]
use crate::MockHardwareTrackerClient;
use crate::{HardwareTrackerClient, HardwareTrackerClientImpl};

#[derive(Clone, Debug)]
pub(crate) enum HardwareTrackerClientFacade {
    Real(&'static HardwareTrackerClientImpl),

    #[cfg(test)]
    Mock(Arc<MockHardwareTrackerClient>),
}

impl HardwareTrackerClientFacade {
    pub(crate) const fn real() -> Self {
        Self::Real(&HardwareTrackerClientImpl)
    }

    #[cfg(test)]
    pub(crate) fn from_mock(mock: MockHardwareTrackerClient) -> Self {
        Self::Mock(Arc::new(mock))
    }
}

impl HardwareTrackerClient for HardwareTrackerClientFacade {
    fn current_memory_region_id(&self) -> MemoryRegionId {
        match self {
            Self::Real(real) => real.current_memory_region_id(),
            #[cfg(test)]
            Self::Mock(mock) => mock.current_memory_region_id(),
        }
    }

    fn is_thread_memory_region_pinned(&self) -> bool {
        match self {
            Self::Real(real) => real.is_thread_memory_region_pinned(),
            #[cfg(test)]
            Self::Mock(mock) => mock.is_thread_memory_region_pinned(),
        }
    }
}
