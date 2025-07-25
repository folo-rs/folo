//! Real platform implementation using system calls.

use std::time::Duration;

use cpu_time::{ProcessTime, ThreadTime};

use crate::pal::abstractions::Platform;

/// Real implementation of the platform abstraction using the `cpu_time` package.
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
