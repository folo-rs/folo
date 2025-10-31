use std::fmt::Debug;
use std::time::Instant;

#[cfg(test)]
use crate::pal::MockTimeSource;
use crate::pal::RustTimeSource;
use crate::pal::TimeSource;
#[cfg(all(any(target_os = "linux", windows), not(miri)))]
use crate::pal::TimeSourceImpl;

pub(crate) enum TimeSourceFacade {
    #[cfg(all(any(target_os = "linux", windows), not(miri)))]
    Real(TimeSourceImpl),

    Rust(RustTimeSource),

    #[cfg(test)]
    Mock(MockTimeSource),
}

#[cfg(all(any(target_os = "linux", windows), not(miri)))]
impl From<TimeSourceImpl> for TimeSourceFacade {
    fn from(ts: TimeSourceImpl) -> Self {
        Self::Real(ts)
    }
}

impl From<RustTimeSource> for TimeSourceFacade {
    fn from(ts: RustTimeSource) -> Self {
        Self::Rust(ts)
    }
}

#[cfg(test)]
impl From<MockTimeSource> for TimeSourceFacade {
    fn from(ts: MockTimeSource) -> Self {
        Self::Mock(ts)
    }
}

impl TimeSource for TimeSourceFacade {
    fn now(&mut self) -> Instant {
        match self {
            #[cfg(all(any(target_os = "linux", windows), not(miri)))]
            Self::Real(ts) => ts.now(),
            Self::Rust(ts) => ts.now(),
            #[cfg(test)]
            Self::Mock(ts) => ts.now(),
        }
    }
}

impl Debug for TimeSourceFacade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(all(any(target_os = "linux", windows), not(miri)))]
            Self::Real(ts) => ts.fmt(f),
            Self::Rust(ts) => ts.fmt(f),
            #[cfg(test)]
            Self::Mock(ts) => ts.fmt(f),
        }
    }
}
