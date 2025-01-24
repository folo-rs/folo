use std::{fmt::Debug, io, mem};

use libc::cpu_set_t;

/// Bindings for FFI calls into external libraries (either provided by operating system or not).
///
/// All PAL FFI calls must go through this trait, enabling them to be mocked.
#[cfg_attr(test, mockall::automock)]
pub(crate) trait Bindings: Debug + Send + Sync + 'static {
    // sched_setaffinity() for the current thread
    fn sched_setaffinity_current(&self, cpuset: &cpu_set_t) -> Result<(), io::Error>;
}

/// FFI bindings that target the real operating system that the build is targeting.
///
/// You would only use different bindings in PAL unit tests that need to use mock bindings.
/// Even then, whenever possible, unit tests should use real bindings for maximum realism.
#[derive(Debug, Default)]
pub(crate) struct BuildTargetBindings;

impl Bindings for BuildTargetBindings {
    fn sched_setaffinity_current(&self, cpuset: &cpu_set_t) -> Result<(), io::Error> {
        // 0 means current thread.
        // SAFETY: No safety requirements beyond passing valid arguments.
        let result = unsafe { libc::sched_setaffinity(0, mem::size_of::<cpu_set_t>(), cpuset) };

        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(result))
        }
    }
}
