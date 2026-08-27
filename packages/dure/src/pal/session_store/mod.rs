//! Session store PAL.

mod abstractions;
mod facade;
mod real;
// The Windows PAL is the operating-system boundary: a thin binding layer over
// Win32 whose failure paths need real operating-system faults to reach. It is
// exercised end to end by the integration tests rather than line by line, and
// is excluded from mutation testing for the same reason (scripts/build/Mutants.psm1).
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(windows)]
mod windows;

pub(crate) use abstractions::*;
pub(crate) use facade::*;
pub(crate) use real::*;
