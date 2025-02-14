use derive_more::derive::Display;

use crate::pal::{AbstractProcessor, ProcessorImpl};

#[cfg(test)]
use crate::pal::FakeProcessor;

#[derive(Clone, Copy, Debug, Display, Eq, Hash, PartialEq)]
pub(crate) enum ProcessorFacade {
    Real(ProcessorImpl),

    #[cfg(test)]
    Fake(FakeProcessor),
}

impl ProcessorFacade {
    pub(crate) fn as_real(&self) -> &ProcessorImpl {
        match self {
            ProcessorFacade::Real(p) => p,
            #[cfg(test)]
            _ => panic!("attempted to dereference facade into wrong type"),
        }
    }
}

impl AsRef<ProcessorFacade> for ProcessorFacade {
    fn as_ref(&self) -> &ProcessorFacade {
        self
    }
}

impl AbstractProcessor for ProcessorFacade {
    fn id(&self) -> crate::ProcessorId {
        match self {
            ProcessorFacade::Real(p) => p.id(),
            #[cfg(test)]
            ProcessorFacade::Fake(p) => p.id(),
        }
    }

    fn memory_region_id(&self) -> crate::MemoryRegionId {
        match self {
            ProcessorFacade::Real(p) => p.memory_region_id(),
            #[cfg(test)]
            ProcessorFacade::Fake(p) => p.memory_region_id(),
        }
    }

    fn efficiency_class(&self) -> crate::EfficiencyClass {
        match self {
            ProcessorFacade::Real(p) => p.efficiency_class(),
            #[cfg(test)]
            ProcessorFacade::Fake(p) => p.efficiency_class(),
        }
    }
}

impl From<ProcessorImpl> for ProcessorFacade {
    fn from(p: ProcessorImpl) -> Self {
        ProcessorFacade::Real(p)
    }
}

#[cfg(test)]
impl From<FakeProcessor> for ProcessorFacade {
    fn from(p: FakeProcessor) -> Self {
        ProcessorFacade::Fake(p)
    }
}
