use std::{fmt::Debug, io, mem};

use libc::cpu_set_t;

use crate::pal::linux::Bindings;

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
            Err(io::Error::last_os_error())
        }
    }

    fn sched_getcpu(&self) -> i32 {
        // SAFETY: No safety requirements.
        unsafe { libc::sched_getcpu() }
    }
}
