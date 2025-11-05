use std::fmt::Debug;
#[cfg(test)]
use std::sync::Arc;

#[cfg(test)]
use crate::pal::MockPlatform;
#[cfg(any(miri, not(any(target_os = "linux", windows))))]
use crate::pal::RustPlatform;
#[cfg(all(any(target_os = "linux", windows), not(miri)))]
use crate::pal::{BUILD_TARGET_PLATFORM, BuildTargetPlatform};
use crate::pal::{Platform, TimeSourceFacade};

#[derive(Clone)]
pub(crate) enum PlatformFacade {
    #[cfg(all(any(target_os = "linux", windows), not(miri)))]
    Optimized(&'static BuildTargetPlatform),

    #[cfg(any(miri, not(any(target_os = "linux", windows))))]
    Passthrough(&'static RustPlatform),

    #[cfg(test)]
    Mock(Arc<MockPlatform>),
}

impl PlatformFacade {
    #[cfg(all(any(target_os = "linux", windows), not(miri)))]
    pub(crate) fn real() -> Self {
        Self::Optimized(&BUILD_TARGET_PLATFORM)
    }

    #[cfg(any(miri, not(any(target_os = "linux", windows))))]
    pub(crate) fn rust() -> Self {
        Self::Passthrough(&RustPlatform)
    }
}

impl Platform for PlatformFacade {
    type TimeSource = TimeSourceFacade;

    fn new_time_source(&self) -> TimeSourceFacade {
        match self {
            #[cfg(all(any(target_os = "linux", windows), not(miri)))]
            Self::Optimized(p) => p.new_time_source().into(),
            #[cfg(any(miri, not(any(target_os = "linux", windows))))]
            Self::Passthrough(p) => p.new_time_source().into(),
            #[cfg(test)]
            Self::Mock(p) => p.new_time_source().into(),
        }
    }
}

#[cfg(all(any(target_os = "linux", windows), not(miri)))]
impl From<&'static BuildTargetPlatform> for PlatformFacade {
    fn from(p: &'static BuildTargetPlatform) -> Self {
        Self::Optimized(p)
    }
}

#[cfg(any(miri, not(any(target_os = "linux", windows))))]
impl From<&'static RustPlatform> for PlatformFacade {
    fn from(p: &'static RustPlatform) -> Self {
        Self::Passthrough(p)
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
            #[cfg(all(any(target_os = "linux", windows), not(miri)))]
            Self::Optimized(p) => p.fmt(f),
            #[cfg(any(miri, not(any(target_os = "linux", windows))))]
            Self::Passthrough(p) => p.fmt(f),
            #[cfg(test)]
            Self::Mock(p) => p.fmt(f),
        }
    }
}
