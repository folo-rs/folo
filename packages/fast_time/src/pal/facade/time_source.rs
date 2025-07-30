use std::fmt::Debug;
use std::time::Instant;

#[cfg(test)]
use crate::pal::MockTimeSource;
use crate::pal::{TimeSource, TimeSourceImpl};

pub(crate) enum TimeSourceFacade {
    Real(TimeSourceImpl),

    #[cfg(test)]
    Mock(MockTimeSource),
}

impl From<TimeSourceImpl> for TimeSourceFacade {
    fn from(ts: TimeSourceImpl) -> Self {
        Self::Real(ts)
    }
}

#[cfg(test)]
impl From<MockTimeSource> for TimeSourceFacade {
    fn from(ts: MockTimeSource) -> Self {
        Self::Mock(ts)
    }
}

impl TimeSource for TimeSourceFacade {
    fn now(&self) -> Instant {
        match self {
            Self::Real(ts) => ts.now(),
            #[cfg(test)]
            Self::Mock(ts) => ts.now(),
        }
    }
}

impl Debug for TimeSourceFacade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Real(ts) => ts.fmt(f),
            #[cfg(test)]
            Self::Mock(ts) => ts.fmt(f),
        }
    }
}
