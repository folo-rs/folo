//! Platform facade for switching between real and fake implementations.

use std::time::Duration;

use crate::pal::abstractions::Platform;
#[cfg(test)]
use crate::pal::fake::FakePlatform;
use crate::pal::real::RealPlatform;

/// Facade that allows switching between real and fake platform implementations.
///
/// This enum provides a unified interface to either the real platform
/// (using actual system calls) or fake platform (for testing).
#[derive(Debug, Clone)]
pub(crate) enum PlatformFacade {
    /// Real platform implementation using system calls.
    Real(RealPlatform),

    /// Fake platform implementation for testing.
    #[cfg(test)]
    Fake(FakePlatform),
}

impl PlatformFacade {
    /// Creates a new platform facade using the real implementation.
    pub(crate) fn real() -> Self {
        Self::Real(RealPlatform)
    }

    /// Creates a new platform facade using the fake implementation.
    #[cfg(test)]
    pub(crate) fn fake(fake_platform: FakePlatform) -> Self {
        Self::Fake(fake_platform)
    }
}

impl Platform for PlatformFacade {
    fn thread_time(&self) -> Duration {
        match self {
            Self::Real(platform) => platform.thread_time(),
            #[cfg(test)]
            Self::Fake(platform) => platform.thread_time(),
        }
    }

    fn process_time(&self) -> Duration {
        match self {
            Self::Real(platform) => platform.process_time(),
            #[cfg(test)]
            Self::Fake(platform) => platform.process_time(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_facade_real() {
        let facade = PlatformFacade::real();
        matches!(facade, PlatformFacade::Real(_));
    }

    #[test]
    fn platform_facade_fake() {
        let fake_platform = FakePlatform::new();
        let facade = PlatformFacade::fake(fake_platform);
        matches!(facade, PlatformFacade::Fake(_));
    }

    #[test]
    fn platform_facade_thread_time() {
        let mut fake_platform = FakePlatform::new();
        fake_platform.set_thread_time(Duration::from_millis(300));
        let facade = PlatformFacade::fake(fake_platform);

        let time = facade.thread_time();
        assert_eq!(time, Duration::from_millis(300));
    }

    #[test]
    fn platform_facade_process_time() {
        let mut fake_platform = FakePlatform::new();
        fake_platform.set_process_time(Duration::from_millis(400));
        let facade = PlatformFacade::fake(fake_platform);

        let time = facade.process_time();
        assert_eq!(time, Duration::from_millis(400));
    }
}
