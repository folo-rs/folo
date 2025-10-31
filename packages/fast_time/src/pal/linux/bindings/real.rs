use std::time::Instant;
use std::{io, mem};

use libc::{CLOCK_MONOTONIC_COARSE, timespec};

use crate::pal::linux::Bindings;

/// FFI bindings that target the real operating system that the build is targeting.
///
/// You would only use different bindings in PAL unit tests that need to use mock bindings.
/// Even then, whenever possible, unit tests should use real bindings for maximum realism.
#[derive(Debug, Default)]
pub(crate) struct BuildTargetBindings;

impl Bindings for BuildTargetBindings {
    /// Returns the current monotonic time in nanoseconds.
    ///
    /// Uses `CLOCK_MONOTONIC_COARSE` which provides a faster but less precise clock
    /// compared to `CLOCK_MONOTONIC`. This is appropriate for high-frequency timestamp
    /// capture where absolute precision is less important than low overhead.
    ///
    /// The arithmetic is guaranteed not to overflow because:
    /// - `tv_sec` represents seconds since an epoch and will not exceed the range of u128
    ///   for any realistic timestamp within the lifespan of the universe.
    /// - The multiplication by `1_000_000_000` converts seconds to nanoseconds.
    #[expect(
        clippy::cast_sign_loss,
        clippy::arithmetic_side_effects,
        reason = "never going to happen with timestamps within real-universe ranges"
    )]
    fn clock_gettime_nanos(&self) -> u128 {
        // SAFETY: All-zero is a valid initial value for this type.
        let mut ts: timespec = unsafe { mem::zeroed() };

        // SAFETY: We are passing valid arguments, no other safety requirements.
        let result = unsafe { libc::clock_gettime(CLOCK_MONOTONIC_COARSE, &raw mut ts) };

        assert!(result == 0, "{}", io::Error::last_os_error());

        ts.tv_sec as u128 * 1_000_000_000 + ts.tv_nsec as u128
    }

    fn now(&self) -> Instant {
        Instant::now()
    }
}
