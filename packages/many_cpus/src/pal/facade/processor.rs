use std::fmt::Debug;

use derive_more::derive::Display;

#[cfg(test)]
use crate::pal::FakeProcessor;
use crate::pal::{AbstractProcessor, ProcessorImpl};

#[derive(Clone, Copy, Display, Eq, Hash, PartialEq)]
pub(crate) enum ProcessorFacade {
    Real(ProcessorImpl),

    #[cfg(test)]
    Fake(FakeProcessor),
}

impl ProcessorFacade {
    pub(crate) fn as_real(&self) -> &ProcessorImpl {
        match self {
            Self::Real(p) => p,
            #[cfg(test)]
            _ => panic!("attempted to dereference facade into wrong type"),
        }
    }
}

impl AsRef<Self> for ProcessorFacade {
    fn as_ref(&self) -> &Self {
        self
    }
}

impl AbstractProcessor for ProcessorFacade {
    fn id(&self) -> crate::ProcessorId {
        match self {
            Self::Real(p) => p.id(),
            #[cfg(test)]
            Self::Fake(p) => p.id(),
        }
    }

    fn memory_region_id(&self) -> crate::MemoryRegionId {
        match self {
            Self::Real(p) => p.memory_region_id(),
            #[cfg(test)]
            Self::Fake(p) => p.memory_region_id(),
        }
    }

    fn efficiency_class(&self) -> crate::EfficiencyClass {
        match self {
            Self::Real(p) => p.efficiency_class(),
            #[cfg(test)]
            Self::Fake(p) => p.efficiency_class(),
        }
    }
}

impl From<ProcessorImpl> for ProcessorFacade {
    fn from(p: ProcessorImpl) -> Self {
        Self::Real(p)
    }
}

#[cfg(test)]
impl From<FakeProcessor> for ProcessorFacade {
    fn from(p: FakeProcessor) -> Self {
        Self::Fake(p)
    }
}

impl Debug for ProcessorFacade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Real(inner) => inner.fmt(f),
            #[cfg(test)]
            Self::Fake(inner) => inner.fmt(f),
        }
    }
}
