#![cfg_attr(coverage_nightly, coverage(off))]

use std::fmt::{self, Debug};
#[cfg(test)]
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[cfg(test)]
use crate::pal::MockTimeSource;
#[cfg(any(miri, not(any(target_os = "linux", windows))))]
use crate::pal::RustTimeSource;
use crate::pal::TimeSource;
#[cfg(all(any(target_os = "linux", windows), not(miri)))]
use crate::pal::TimeSourceImpl;

#[derive(Clone)]
pub(crate) enum TimeSourceFacade {
    #[cfg(all(any(target_os = "linux", windows), not(miri)))]
    Optimized(TimeSourceImpl),

    #[cfg(any(miri, not(any(target_os = "linux", windows))))]
    Passthrough(RustTimeSource),

    #[cfg(test)]
    Mock(Arc<Mutex<MockTimeSource>>),
}

#[cfg(all(any(target_os = "linux", windows), not(miri)))]
impl From<TimeSourceImpl> for TimeSourceFacade {
    fn from(ts: TimeSourceImpl) -> Self {
        Self::Optimized(ts)
    }
}

#[cfg(any(miri, not(any(target_os = "linux", windows))))]
impl From<RustTimeSource> for TimeSourceFacade {
    fn from(ts: RustTimeSource) -> Self {
        Self::Passthrough(ts)
    }
}

#[cfg(test)]
impl From<MockTimeSource> for TimeSourceFacade {
    fn from(ts: MockTimeSource) -> Self {
        Self::Mock(Arc::new(Mutex::new(ts)))
    }
}

impl TimeSource for TimeSourceFacade {
    fn now(&mut self) -> Instant {
        match self {
            #[cfg(all(any(target_os = "linux", windows), not(miri)))]
            Self::Optimized(ts) => ts.now(),
            #[cfg(any(miri, not(any(target_os = "linux", windows))))]
            Self::Passthrough(ts) => ts.now(),
            #[cfg(test)]
            Self::Mock(ts) => ts
                .lock()
                .expect("mock time source does not support operation after panic in mock")
                .now(),
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl Debug for TimeSourceFacade {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            #[cfg(all(any(target_os = "linux", windows), not(miri)))]
            Self::Optimized(ts) => ts.fmt(f),
            #[cfg(any(miri, not(any(target_os = "linux", windows))))]
            Self::Passthrough(ts) => ts.fmt(f),
            #[cfg(test)]
            Self::Mock(ts) => ts.fmt(f),
        }
    }
}
