use std::fmt::Debug;

use derive_more::derive::Display;

#[cfg(test)]
use crate::pal::FakeProcessor;
#[cfg(test)]
use crate::pal::fallback::ProcessorImpl as FallbackProcessor;
use crate::pal::{AbstractProcessor, ProcessorImpl};

#[derive(Clone, Copy, Display, Eq, Hash, PartialEq)]
pub(crate) enum ProcessorFacade {
    Target(ProcessorImpl),

    #[cfg(test)]
    Fallback(FallbackProcessor),

    #[cfg(test)]
    Fake(FakeProcessor),
}

impl ProcessorFacade {
    // Only available on platforms with native support. The fallback platform does not need
    // direct access to the underlying ProcessorImpl type.
    #[cfg(any(windows, target_os = "linux"))]
    pub(crate) fn as_target(&self) -> &ProcessorImpl {
        match self {
            Self::Target(p) => p,
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
            Self::Target(p) => p.id(),
            #[cfg(test)]
            Self::Fallback(p) => p.id(),
            #[cfg(test)]
            Self::Fake(p) => p.id(),
        }
    }

    fn memory_region_id(&self) -> crate::MemoryRegionId {
        match self {
            Self::Target(p) => p.memory_region_id(),
            #[cfg(test)]
            Self::Fallback(p) => p.memory_region_id(),
            #[cfg(test)]
            Self::Fake(p) => p.memory_region_id(),
        }
    }

    fn efficiency_class(&self) -> crate::EfficiencyClass {
        match self {
            Self::Target(p) => p.efficiency_class(),
            #[cfg(test)]
            Self::Fallback(p) => p.efficiency_class(),
            #[cfg(test)]
            Self::Fake(p) => p.efficiency_class(),
        }
    }
}

impl From<ProcessorImpl> for ProcessorFacade {
    fn from(p: ProcessorImpl) -> Self {
        Self::Target(p)
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
            Self::Target(inner) => inner.fmt(f),
            #[cfg(test)]
            Self::Fallback(inner) => inner.fmt(f),
            #[cfg(test)]
            Self::Fake(inner) => inner.fmt(f),
        }
    }
}
