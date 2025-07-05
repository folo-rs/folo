//! Real platform implementation using system calls.

use crate::pal::abstractions::Platform;
use cpu_time::{ProcessTime, ThreadTime};
use std::time::Duration;

/// Real implementation of the platform abstraction using the `cpu_time` crate.
#[derive(Clone, Debug)]
pub(crate) struct RealPlatform;

impl Platform for RealPlatform {
    fn thread_time(&self) -> Duration {
        ThreadTime::now().as_duration()
    }

    fn process_time(&self) -> Duration {
        ProcessTime::now().as_duration()
    }
}
