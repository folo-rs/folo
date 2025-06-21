#[cfg(test)]
use std::sync::Arc;

#[cfg(test)]
use crate::MockHardwareTrackerClient;
use crate::{HardwareTrackerClient, HardwareTrackerClientImpl, MemoryRegionId, ProcessorId};

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

    #[cfg(test)]
    pub(crate) fn default_mock() -> Self {
        Self::Mock(Arc::new(MockHardwareTrackerClient::new()))
    }
}

impl HardwareTrackerClient for HardwareTrackerClientFacade {
    fn update_pin_status(
        &self,
        processor_id: Option<ProcessorId>,
        memory_region_id: Option<MemoryRegionId>,
    ) {
        match self {
            Self::Real(real) => {
                real.update_pin_status(processor_id, memory_region_id);
            }
            #[cfg(test)]
            Self::Mock(mock) => {
                mock.update_pin_status(processor_id, memory_region_id);
            }
        }
    }
}
