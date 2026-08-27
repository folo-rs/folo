//! Transport PAL.

mod abstractions;
mod facade;
#[cfg(test)]
mod memory;
#[cfg(not(windows))]
mod unsupported;
#[cfg(windows)]
mod windows;

pub(crate) use abstractions::*;
pub(crate) use facade::*;
#[cfg(test)]
pub(crate) use memory::*;
#[cfg(not(windows))]
pub(crate) use unsupported::BuildTargetTransport;
#[cfg(windows)]
pub(crate) use windows::BuildTargetTransport;
