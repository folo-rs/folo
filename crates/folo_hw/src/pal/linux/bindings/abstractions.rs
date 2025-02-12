use std::{fmt::Debug, io};

use libc::cpu_set_t;

/// Bindings for FFI calls into external libraries (either provided by operating system or not).
///
/// All PAL FFI calls must go through this trait, enabling them to be mocked.
#[cfg_attr(test, mockall::automock)]
pub(crate) trait Bindings: Debug + Send + Sync + 'static {
    // sched_setaffinity() for the current thread
    fn sched_setaffinity_current(&self, cpuset: &cpu_set_t) -> Result<(), io::Error>;
}
