use std::fmt::Debug;

#[cfg(test)]
use std::sync::Arc;

use crate::pal::{BUILD_TARGET_PLATFORM, BuildTargetPlatform, Platform, ProcessorFacade};

#[cfg(test)]
use crate::pal::MockPlatform;

#[derive(Clone)]
pub(crate) enum PlatformFacade {
    Real(&'static BuildTargetPlatform),

    #[cfg(test)]
    Mock(Arc<MockPlatform>),
}

impl PlatformFacade {
    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    pub(crate) fn real() -> Self {
        PlatformFacade::Real(&BUILD_TARGET_PLATFORM)
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    #[cfg(test)]
    pub(crate) fn from_mock(mock: MockPlatform) -> Self {
        PlatformFacade::Mock(Arc::new(mock))
    }
}

impl Platform for PlatformFacade {
    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn get_all_processors(&self) -> nonempty::NonEmpty<ProcessorFacade> {
        match self {
            PlatformFacade::Real(p) => p.get_all_processors(),
            #[cfg(test)]
            PlatformFacade::Mock(p) => p.get_all_processors(),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn pin_current_thread_to<P>(&self, processors: &nonempty::NonEmpty<P>)
    where
        P: AsRef<ProcessorFacade>,
    {
        match self {
            PlatformFacade::Real(p) => p.pin_current_thread_to(processors),
            #[cfg(test)]
            PlatformFacade::Mock(p) => p.pin_current_thread_to(processors),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn current_processor_id(&self) -> crate::ProcessorId {
        match self {
            PlatformFacade::Real(p) => p.current_processor_id(),
            #[cfg(test)]
            PlatformFacade::Mock(p) => p.current_processor_id(),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn max_processor_id(&self) -> crate::ProcessorId {
        match self {
            PlatformFacade::Real(p) => p.max_processor_id(),
            #[cfg(test)]
            PlatformFacade::Mock(p) => p.max_processor_id(),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn max_memory_region_id(&self) -> crate::MemoryRegionId {
        match self {
            PlatformFacade::Real(p) => p.max_memory_region_id(),
            #[cfg(test)]
            PlatformFacade::Mock(p) => p.max_memory_region_id(),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn current_thread_processors(&self) -> nonempty::NonEmpty<crate::ProcessorId> {
        match self {
            PlatformFacade::Real(p) => p.current_thread_processors(),
            #[cfg(test)]
            PlatformFacade::Mock(p) => p.current_thread_processors(),
        }
    }
}

impl From<&'static BuildTargetPlatform> for PlatformFacade {
    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn from(p: &'static BuildTargetPlatform) -> Self {
        PlatformFacade::Real(p)
    }
}

#[cfg(test)]
impl From<MockPlatform> for PlatformFacade {
    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn from(p: MockPlatform) -> Self {
        PlatformFacade::Mock(Arc::new(p))
    }
}

impl Debug for PlatformFacade {
    #[cfg_attr(test, mutants::skip)] // Trivial layer, mutation not insightful.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Real(inner) => inner.fmt(f),
            #[cfg(test)]
            Self::Mock(inner) => inner.fmt(f),
        }
    }
}
