use many_cpus::{HardwareInfo, MemoryRegionId};

#[cfg_attr(test, mockall::automock)]
pub(crate) trait HardwareInfoClient {
    fn max_memory_region_id(&self) -> MemoryRegionId;
}

#[derive(Debug)]
pub(crate) struct HardwareInfoClientImpl;

impl HardwareInfoClient for HardwareInfoClientImpl {
    fn max_memory_region_id(&self) -> MemoryRegionId {
        HardwareInfo::current().max_memory_region_id()
    }
}

#[derive(Debug)]
pub(crate) enum HardwareInfoClientFacade {
    Real(&'static HardwareInfoClientImpl),

    #[cfg(test)]
    Mock(MockHardwareInfoClient),
}

impl HardwareInfoClientFacade {
    pub(crate) const fn real() -> Self {
        Self::Real(&HardwareInfoClientImpl)
    }

    #[cfg(test)]
    pub(crate) fn from_mock(mock: MockHardwareInfoClient) -> Self {
        Self::Mock(mock)
    }
}

impl HardwareInfoClient for HardwareInfoClientFacade {
    fn max_memory_region_id(&self) -> MemoryRegionId {
        match self {
            HardwareInfoClientFacade::Real(real) => real.max_memory_region_id(),
            #[cfg(test)]
            HardwareInfoClientFacade::Mock(mock) => mock.max_memory_region_id(),
        }
    }
}
