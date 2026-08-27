//! Process PAL.

mod abstractions;
mod command_line;
mod facade;
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
#[cfg(windows)]
pub(crate) use command_line::*;
pub(crate) use facade::*;
#[cfg(not(windows))]
pub(crate) use unsupported::BuildTargetProcesses;
#[cfg(windows)]
pub(crate) use windows::BuildTargetProcesses;
