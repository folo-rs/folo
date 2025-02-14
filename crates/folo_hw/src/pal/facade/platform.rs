#[cfg(test)]
use std::sync::Arc;

use crate::pal::{BuildTargetPlatform, Platform, ProcessorFacade, BUILD_TARGET_PLATFORM};

#[cfg(test)]
use crate::pal::MockPlatform;

#[derive(Clone, Debug)]
pub(crate) enum PlatformFacade {
    Real(&'static BuildTargetPlatform),

    #[cfg(test)]
    Mock(Arc<MockPlatform>),
}

impl PlatformFacade {
    pub(crate) fn real() -> Self {
        PlatformFacade::Real(&BUILD_TARGET_PLATFORM)
    }

    #[cfg(test)]
    pub(crate) fn from_mock(mock: MockPlatform) -> Self {
        PlatformFacade::Mock(Arc::new(mock))
    }
}

impl Platform for PlatformFacade {
    fn get_all_processors(&self) -> nonempty::NonEmpty<ProcessorFacade> {
        match self {
            PlatformFacade::Real(p) => p.get_all_processors(),
            #[cfg(test)]
            PlatformFacade::Mock(p) => p.get_all_processors(),
        }
    }

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

    fn current_processor_id(&self) -> crate::ProcessorId {
        match self {
            PlatformFacade::Real(p) => p.current_processor_id(),
            #[cfg(test)]
            PlatformFacade::Mock(p) => p.current_processor_id(),
        }
    }
}

impl From<&'static BuildTargetPlatform> for PlatformFacade {
    fn from(p: &'static BuildTargetPlatform) -> Self {
        PlatformFacade::Real(p)
    }
}

#[cfg(test)]
impl From<MockPlatform> for PlatformFacade {
    fn from(p: MockPlatform) -> Self {
        PlatformFacade::Mock(Arc::new(p))
    }
}
