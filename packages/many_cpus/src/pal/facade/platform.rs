#![cfg_attr(coverage_nightly, coverage(off))]

use std::fmt::Debug;
#[cfg(any(test, feature = "test-util"))]
use std::sync::Arc;

#[cfg(any(test, feature = "test-util"))]
use crate::fake::FakePlatform;
#[cfg(test)]
use crate::pal::fallback::BuildTargetPlatform as FallbackPlatform;
use crate::pal::{BUILD_TARGET_PLATFORM, BuildTargetPlatform, Platform, ProcessorFacade};

#[derive(Clone)]
pub(crate) enum PlatformFacade {
    Target(&'static BuildTargetPlatform),

    #[cfg(test)]
    Fallback(&'static FallbackPlatform),

    /// Fake hardware for the public test-util feature.
    #[cfg(any(test, feature = "test-util"))]
    Fake(Arc<FakePlatform>),
}

impl PlatformFacade {
    pub(crate) fn target() -> Self {
        Self::Target(&BUILD_TARGET_PLATFORM)
    }

    #[cfg(any(test, feature = "test-util"))]
    pub(crate) fn from_fake(fake: FakePlatform) -> Self {
        Self::Fake(Arc::new(fake))
    }

    /// Returns a reference to the inner `FakePlatform`, panicking if not a fake.
    #[cfg(test)]
    pub(crate) fn as_fake(&self) -> &FakePlatform {
        match self {
            Self::Fake(inner) => inner,
            _ => panic!("expected PlatformFacade::Fake"),
        }
    }
}

impl Platform for PlatformFacade {
    fn get_all_processors(&self) -> nonempty::NonEmpty<ProcessorFacade> {
        match self {
            Self::Target(p) => p.get_all_processors(),
            #[cfg(test)]
            Self::Fallback(p) => p.get_all_processors(),
            #[cfg(any(test, feature = "test-util"))]
            Self::Fake(p) => p.get_all_processors(),
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
            #[cfg(any(test, feature = "test-util"))]
            Self::Fake(p) => p.pin_current_thread_to(processors),
        }
    }

    fn current_processor_id(&self) -> crate::ProcessorId {
        match self {
            Self::Target(p) => p.current_processor_id(),
            #[cfg(test)]
            Self::Fallback(p) => p.current_processor_id(),
            #[cfg(any(test, feature = "test-util"))]
            Self::Fake(p) => p.current_processor_id(),
        }
    }

    fn max_processor_id(&self) -> crate::ProcessorId {
        match self {
            Self::Target(p) => p.max_processor_id(),
            #[cfg(test)]
            Self::Fallback(p) => p.max_processor_id(),
            #[cfg(any(test, feature = "test-util"))]
            Self::Fake(p) => p.max_processor_id(),
        }
    }

    fn max_memory_region_id(&self) -> crate::MemoryRegionId {
        match self {
            Self::Target(p) => p.max_memory_region_id(),
            #[cfg(test)]
            Self::Fallback(p) => p.max_memory_region_id(),
            #[cfg(any(test, feature = "test-util"))]
            Self::Fake(p) => p.max_memory_region_id(),
        }
    }

    fn current_thread_processors(&self) -> nonempty::NonEmpty<crate::ProcessorId> {
        match self {
            Self::Target(p) => p.current_thread_processors(),
            #[cfg(test)]
            Self::Fallback(p) => p.current_thread_processors(),
            #[cfg(any(test, feature = "test-util"))]
            Self::Fake(p) => p.current_thread_processors(),
        }
    }

    fn max_processor_time(&self) -> f64 {
        match self {
            Self::Target(p) => p.max_processor_time(),
            #[cfg(test)]
            Self::Fallback(p) => p.max_processor_time(),
            #[cfg(any(test, feature = "test-util"))]
            Self::Fake(p) => p.max_processor_time(),
        }
    }

    fn active_processor_count(&self) -> usize {
        match self {
            Self::Target(p) => p.active_processor_count(),
            #[cfg(test)]
            Self::Fallback(p) => p.active_processor_count(),
            #[cfg(any(test, feature = "test-util"))]
            Self::Fake(p) => p.active_processor_count(),
        }
    }
}

impl From<&'static BuildTargetPlatform> for PlatformFacade {
    fn from(p: &'static BuildTargetPlatform) -> Self {
        Self::Target(p)
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl Debug for PlatformFacade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Target(inner) => inner.fmt(f),
            #[cfg(test)]
            Self::Fallback(inner) => inner.fmt(f),
            #[cfg(any(test, feature = "test-util"))]
            Self::Fake(inner) => inner.fmt(f),
        }
    }
}
