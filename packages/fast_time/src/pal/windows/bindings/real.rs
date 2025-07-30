use std::time::Instant;

use windows::Win32::System::SystemInformation::GetTickCount64;

use crate::pal::windows::Bindings;

/// FFI bindings that target the real operating system that the build is targeting.
///
/// You would only use different bindings in PAL unit tests that need to use mock bindings.
/// Even then, whenever possible, unit tests should use real bindings for maximum realism.
#[derive(Debug, Default)]
pub(crate) struct BuildTargetBindings;

impl Bindings for BuildTargetBindings {
    fn get_tick_count_64(&self) -> u64 {
        // SAFETY: No safety requirements.
        unsafe { GetTickCount64() }
    }

    fn now(&self) -> Instant {
        Instant::now()
    }
}
