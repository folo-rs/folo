//! Local console PAL.

mod abstractions;
mod facade;
// The non-Windows build has no working PAL: this stub only exists so the crate
// still compiles there, and the platform gate refuses to run before any of it is
// reached. It carries no coverage expectations of its own.
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(not(windows))]
mod unsupported;
// The Windows PAL is the operating-system boundary: a thin binding layer over
// Win32 whose failure paths need real operating-system faults to reach. It is
// exercised end to end by the integration tests rather than line by line, and
// is excluded from mutation testing for the same reason (scripts/build/Mutants.psm1).
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(windows)]
mod windows;

pub(crate) use abstractions::*;
pub(crate) use facade::*;
#[cfg(not(windows))]
pub(crate) use unsupported::BuildTargetConsole;
#[cfg(windows)]
pub(crate) use windows::BuildTargetConsole;
