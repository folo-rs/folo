use std::fmt::Debug;
use std::{io, mem};

use libc::cpu_set_t;

use crate::pal::linux::Bindings;

/// FFI bindings that target the real operating system that the build is targeting.
///
/// You would only use different bindings in PAL unit tests that need to use mock bindings.
/// Even then, whenever possible, unit tests should use real bindings for maximum realism.
#[derive(Debug, Default)]
pub(crate) struct BuildTargetBindings;

// Real OS bindings are excluded from coverage measurement because:
// 1. They are tested via integration tests running on actual Linux.
// 2. Error paths require OS-level failures that are impractical to trigger in tests.
#[cfg_attr(coverage_nightly, coverage(off))]
impl Bindings for BuildTargetBindings {
    fn sched_setaffinity_current(&self, cpuset: &cpu_set_t) -> Result<(), io::Error> {
        // 0 means current thread.
        // SAFETY: No safety requirements beyond passing valid arguments.
        let result = unsafe { libc::sched_setaffinity(0, size_of::<cpu_set_t>(), cpuset) };

        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn sched_getcpu(&self) -> i32 {
        // SAFETY: No safety requirements.
        unsafe { libc::sched_getcpu() }
    }

    fn sched_getaffinity_current(&self) -> Result<cpu_set_t, io::Error> {
        // SAFETY: All zeroes is a valid cpu_set_t.
        let mut cpuset: cpu_set_t = unsafe { mem::zeroed() };

        // 0 means current thread.
        // SAFETY: No safety requirements beyond passing valid arguments.
        let result = unsafe { libc::sched_getaffinity(0, size_of::<cpu_set_t>(), &raw mut cpuset) };

        if result == 0 {
            Ok(cpuset)
        } else {
            Err(io::Error::last_os_error())
        }
    }
}
