use std::fmt::Debug;
#[cfg(test)]
use std::sync::Arc;

#[cfg(test)]
use crate::pal::MockPlatform;
use crate::pal::{BUILD_TARGET_PLATFORM, BuildTargetPlatform, Platform, TimeSourceFacade};

#[derive(Clone)]
pub(crate) enum PlatformFacade {
    Real(&'static BuildTargetPlatform),

    #[cfg(test)]
    Mock(Arc<MockPlatform>),
}

impl PlatformFacade {
    pub(crate) fn real() -> Self {
        Self::Real(&BUILD_TARGET_PLATFORM)
    }

    #[cfg(test)]
    pub(crate) fn from_mock(mock: MockPlatform) -> Self {
        Self::Mock(Arc::new(mock))
    }
}

impl Platform for PlatformFacade {
    type TimeSource = TimeSourceFacade;

    fn new_time_source(&self) -> TimeSourceFacade {
        match self {
            Self::Real(p) => p.new_time_source().into(),
            #[cfg(test)]
            Self::Mock(p) => p.new_time_source().into(),
        }
    }
}

impl From<&'static BuildTargetPlatform> for PlatformFacade {
    fn from(p: &'static BuildTargetPlatform) -> Self {
        Self::Real(p)
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
            Self::Real(p) => p.fmt(f),
            #[cfg(test)]
            Self::Mock(p) => p.fmt(f),
        }
    }
}
