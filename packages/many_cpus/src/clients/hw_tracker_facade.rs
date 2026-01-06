// Facade types are trivial pass-through layers - not worth testing.
#![cfg_attr(coverage_nightly, coverage(off))]

#[cfg(test)]
use std::sync::Arc;

#[cfg(test)]
use crate::MockHardwareTrackerClient;
use crate::{HardwareTrackerClient, HardwareTrackerClientImpl, MemoryRegionId, ProcessorId};

#[derive(Clone, Debug)]
pub(crate) enum HardwareTrackerClientFacade {
    Target(&'static HardwareTrackerClientImpl),

    /// A client backed by a specific `SystemHardware` instance.
    /// Used when building processor sets from a `SystemHardware` instance.
    Hardware(crate::SystemHardware),

    #[cfg(test)]
    Mock(Arc<MockHardwareTrackerClient>),
}

impl HardwareTrackerClientFacade {
    pub(crate) const fn target() -> Self {
        Self::Target(&HardwareTrackerClientImpl)
    }

    pub(crate) fn from_hardware(hardware: crate::SystemHardware) -> Self {
        Self::Hardware(hardware)
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
            Self::Target(real) => {
                real.update_pin_status(processor_id, memory_region_id);
            }
            Self::Hardware(hardware) => {
                hardware.update_pin_status(processor_id, memory_region_id);
            }
            #[cfg(test)]
            Self::Mock(mock) => {
                mock.update_pin_status(processor_id, memory_region_id);
            }
        }
    }
}
