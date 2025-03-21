#[cfg(test)]
use std::sync::Arc;

use many_cpus::HardwareInfo;

#[cfg_attr(test, mockall::automock)]
pub(crate) trait HardwareInfoClient {
    fn max_memory_region_count(&self) -> usize;
}

#[derive(Debug)]
pub(crate) struct HardwareInfoClientImpl;

impl HardwareInfoClient for HardwareInfoClientImpl {
    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn max_memory_region_count(&self) -> usize {
        HardwareInfo::current().max_memory_region_count()
    }
}

#[derive(Clone, Debug)]
pub(crate) enum HardwareInfoClientFacade {
    Real(&'static HardwareInfoClientImpl),

    #[cfg(test)]
    Mock(Arc<MockHardwareInfoClient>),
}

impl HardwareInfoClientFacade {
    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    pub(crate) const fn real() -> Self {
        Self::Real(&HardwareInfoClientImpl)
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    #[cfg(test)]
    pub(crate) fn from_mock(mock: MockHardwareInfoClient) -> Self {
        Self::Mock(Arc::new(mock))
    }
}

impl HardwareInfoClient for HardwareInfoClientFacade {
    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn max_memory_region_count(&self) -> usize {
        match self {
            HardwareInfoClientFacade::Real(real) => real.max_memory_region_count(),
            #[cfg(test)]
            HardwareInfoClientFacade::Mock(mock) => mock.max_memory_region_count(),
        }
    }
}
