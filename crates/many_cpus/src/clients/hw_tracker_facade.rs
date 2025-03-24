#[cfg(test)]
use std::sync::Arc;

use crate::{MemoryRegionId, ProcessorId};

use crate::{HardwareTrackerClient, HardwareTrackerClientImpl};

#[cfg(test)]
use crate::MockHardwareTrackerClient;

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
            HardwareTrackerClientFacade::Real(real) => {
                real.update_pin_status(processor_id, memory_region_id)
            }
            #[cfg(test)]
            HardwareTrackerClientFacade::Mock(mock) => {
                mock.update_pin_status(processor_id, memory_region_id)
            }
        }
    }
}
