#[cfg(test)]
use std::sync::Arc;

use crate::{HardwareInfoClient, HardwareInfoClientImpl};

#[cfg(test)]
use crate::MockHardwareInfoClient;

#[derive(Clone, Debug)]
pub(crate) enum HardwareInfoClientFacade {
    Real(&'static HardwareInfoClientImpl),

    #[cfg(test)]
    Mock(Arc<MockHardwareInfoClient>),
}

impl HardwareInfoClientFacade {
    pub(crate) const fn real() -> Self {
        Self::Real(&HardwareInfoClientImpl)
    }

    #[cfg(test)]
    pub(crate) fn from_mock(mock: MockHardwareInfoClient) -> Self {
        Self::Mock(Arc::new(mock))
    }
}

impl HardwareInfoClient for HardwareInfoClientFacade {
    fn max_memory_region_count(&self) -> usize {
        match self {
            Self::Real(real) => real.max_memory_region_count(),
            #[cfg(test)]
            Self::Mock(mock) => mock.max_memory_region_count(),
        }
    }
}
