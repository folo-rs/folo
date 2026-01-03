// Filesystem abstraction for cargo-detect-package.
//
// Provides a mockable interface over filesystem operations used by the package detection logic.

mod abstractions;
mod facade;
mod real;

pub(crate) use abstractions::*;
pub(crate) use facade::*;
pub(crate) use real::*;
