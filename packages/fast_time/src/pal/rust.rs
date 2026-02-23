use std::time::Instant;

use crate::pal::{Platform, TimeSource};

/// We use this under Miri because Miri cannot talk to a real OS but Rust std time still works.
#[derive(Debug)]
#[allow(dead_code, reason = "conditional")]
pub(crate) struct RustPlatform;

impl Platform for RustPlatform {
    type TimeSource = RustTimeSource;

    fn new_time_source(&self) -> Self::TimeSource {
        RustTimeSource
    }
}

#[derive(Clone, Debug)]
#[allow(dead_code, reason = "conditional")]
pub(crate) struct RustTimeSource;

impl TimeSource for RustTimeSource {
    fn now(&mut self) -> Instant {
        Instant::now()
    }
}
