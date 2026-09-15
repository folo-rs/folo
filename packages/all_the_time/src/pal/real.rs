//! Real platform implementation using system calls.

use std::time::Duration;

use cpu_time::{ProcessTime, ThreadTime};

use crate::pal::abstractions::Platform;

/// Real implementation of the platform abstraction using the `cpu_time` package.
#[derive(Clone, Debug)]
pub(crate) struct RealPlatform;

impl Platform for RealPlatform {
    // This only forwards cpu_time's OS clock. Distinguishing a constant replacement requires
    // real-time assertions; deterministic delta coverage belongs above the PAL.
    #[cfg_attr(test, mutants::skip)]
    fn thread_time(&self) -> Duration {
        ThreadTime::now().as_duration()
    }

    // This only forwards cpu_time's OS clock. Distinguishing a constant replacement requires
    // real-time assertions; deterministic delta coverage belongs above the PAL.
    #[cfg_attr(test, mutants::skip)]
    fn process_time(&self) -> Duration {
        ProcessTime::now().as_duration()
    }
}
