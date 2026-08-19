#![cfg_attr(
    test,
    expect(
        clippy::struct_field_names,
        reason = "false positive from automock generated code"
    )
)]

use std::fmt::Debug;
use std::io;
use std::num::NonZero;

use crate::pal::linux::CpuMask;

/// Bindings for FFI calls into external libraries (either provided by operating system or not).
///
/// All PAL FFI calls must go through this trait, enabling them to be mocked.
#[cfg_attr(test, mockall::automock)]
pub(crate) trait Bindings: Debug + Send + Sync + 'static {
    // sched_setaffinity() for the current thread
    fn sched_setaffinity_current(&self, mask: &CpuMask) -> Result<(), io::Error>;

    /// `sched_getaffinity()` for the current thread, into a mask of the requested width.
    ///
    /// The operating system refuses to fill a mask that is too narrow to describe every
    /// processor that it knows of, so the width is the caller's choice to make and to correct.
    fn sched_getaffinity_current(&self, words: NonZero<usize>) -> Result<CpuMask, io::Error>;

    fn sched_getcpu(&self) -> i32;
}
