//! Local console PAL.

mod abstractions;
mod facade;
#[cfg(not(windows))]
mod unsupported;
#[cfg(windows)]
mod windows;

pub(crate) use abstractions::*;
pub(crate) use facade::*;
#[cfg(not(windows))]
pub(crate) use unsupported::BuildTargetConsole;
#[cfg(windows)]
pub(crate) use windows::BuildTargetConsole;
