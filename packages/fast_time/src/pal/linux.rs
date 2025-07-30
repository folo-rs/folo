use std::marker::PhantomData;

use crate::pal::{Platform, TimeSource};

#[derive(Debug)]
pub(crate) struct BuildTargetPlatform {
    _placeholder: PhantomData<()>,
}

impl BuildTargetPlatform {
    pub(crate) const fn new() -> Self {
        Self {
            _placeholder: PhantomData,
        }
    }
}

impl Platform for BuildTargetPlatform {
    type TimeSource = TimeSourceImpl;

    fn new_time_source(&self) -> Self::TimeSource {
        Self::TimeSource::new()
    }
}

#[derive(Debug)]
pub(crate) struct TimeSourceImpl {
    _placeholder: PhantomData<()>,
}

impl TimeSourceImpl {
    pub(crate) const fn new() -> Self {
        Self {
            _placeholder: PhantomData,
        }
    }
}

impl TimeSource for TimeSourceImpl {
    fn now(&self) -> std::time::Instant {
        // TODO: Optimize this.
        // Probably should use clock_gettime(CLOCK_MONOTONIC_COARSE, &ts);
        std::time::Instant::now()
    }
}
