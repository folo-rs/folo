//! Pseudoconsole PAL.

mod abstractions;
mod facade;
// A fake used only to drive unit tests, so it is test infrastructure rather
// than product code and carries no coverage expectations of its own.
#[cfg_attr(coverage_nightly, coverage(off))]
#[cfg(test)]
mod memory;
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
#[cfg(test)]
pub(crate) use memory::*;
#[cfg(not(windows))]
pub(crate) use unsupported::BuildTargetPseudoconsole;
#[cfg(windows)]
pub(crate) use windows::{BuildTargetPseudoconsole, hpcon_for};
