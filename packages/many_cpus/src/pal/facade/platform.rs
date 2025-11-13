use std::fmt::Debug;
#[cfg(test)]
use std::sync::Arc;

#[cfg(test)]
use crate::pal::MockPlatform;
#[cfg(test)]
use crate::pal::fallback::BuildTargetPlatform as FallbackPlatform;
use crate::pal::{BUILD_TARGET_PLATFORM, BuildTargetPlatform, Platform, ProcessorFacade};

#[derive(Clone)]
pub(crate) enum PlatformFacade {
    Target(&'static BuildTargetPlatform),

    #[cfg(test)]
    Fallback(&'static FallbackPlatform),

    #[cfg(test)]
    Mock(Arc<MockPlatform>),
}

impl PlatformFacade {
    pub(crate) fn target() -> Self {
        Self::Target(&BUILD_TARGET_PLATFORM)
    }

    #[cfg(test)]
    pub(crate) fn from_mock(mock: MockPlatform) -> Self {
        Self::Mock(Arc::new(mock))
    }
}

impl Platform for PlatformFacade {
    fn get_all_processors(&self) -> nonempty::NonEmpty<ProcessorFacade> {
        match self {
            Self::Target(p) => p.get_all_processors(),
            #[cfg(test)]
            Self::Fallback(p) => p.get_all_processors(),
            #[cfg(test)]
            Self::Mock(p) => p.get_all_processors(),
        }
    }

    fn pin_current_thread_to<P>(&self, processors: &nonempty::NonEmpty<P>)
    where
        P: AsRef<ProcessorFacade>,
    {
        match self {
            Self::Target(p) => p.pin_current_thread_to(processors),
            #[cfg(test)]
            Self::Fallback(p) => p.pin_current_thread_to(processors),
            #[cfg(test)]
            Self::Mock(p) => p.pin_current_thread_to(processors),
        }
    }

    fn current_processor_id(&self) -> crate::ProcessorId {
        match self {
            Self::Target(p) => p.current_processor_id(),
            #[cfg(test)]
            Self::Fallback(p) => p.current_processor_id(),
            #[cfg(test)]
            Self::Mock(p) => p.current_processor_id(),
        }
    }

    fn max_processor_id(&self) -> crate::ProcessorId {
        match self {
            Self::Target(p) => p.max_processor_id(),
            #[cfg(test)]
            Self::Fallback(p) => p.max_processor_id(),
            #[cfg(test)]
            Self::Mock(p) => p.max_processor_id(),
        }
    }

    fn max_memory_region_id(&self) -> crate::MemoryRegionId {
        match self {
            Self::Target(p) => p.max_memory_region_id(),
            #[cfg(test)]
            Self::Fallback(p) => p.max_memory_region_id(),
            #[cfg(test)]
            Self::Mock(p) => p.max_memory_region_id(),
        }
    }

    fn current_thread_processors(&self) -> nonempty::NonEmpty<crate::ProcessorId> {
        match self {
            Self::Target(p) => p.current_thread_processors(),
            #[cfg(test)]
            Self::Fallback(p) => p.current_thread_processors(),
            #[cfg(test)]
            Self::Mock(p) => p.current_thread_processors(),
        }
    }

    fn max_processor_time(&self) -> f64 {
        match self {
            Self::Target(p) => p.max_processor_time(),
            #[cfg(test)]
            Self::Fallback(p) => p.max_processor_time(),
            #[cfg(test)]
            Self::Mock(p) => p.max_processor_time(),
        }
    }

    fn active_processor_count(&self) -> usize {
        match self {
            Self::Target(p) => p.active_processor_count(),
            #[cfg(test)]
            Self::Fallback(p) => p.active_processor_count(),
            #[cfg(test)]
            Self::Mock(p) => p.active_processor_count(),
        }
    }
}

impl From<&'static BuildTargetPlatform> for PlatformFacade {
    fn from(p: &'static BuildTargetPlatform) -> Self {
        Self::Target(p)
    }
}

#[cfg(test)]
impl From<MockPlatform> for PlatformFacade {
    fn from(p: MockPlatform) -> Self {
        Self::Mock(Arc::new(p))
    }
}

impl Debug for PlatformFacade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Target(inner) => inner.fmt(f),
            #[cfg(test)]
            Self::Fallback(inner) => inner.fmt(f),
            #[cfg(test)]
            Self::Mock(inner) => inner.fmt(f),
        }
    }
}
