//! Platform Abstraction Layer (PAL). This is private API, though `pub` in parts to allow
//! benchmark code to bypass public API layers for more accurate benchmarking.

mod abstractions;
pub(crate) use abstractions::*;

mod facade;
pub(crate) use facade::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub(crate) use linux::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(test)]
mod mocks;
#[cfg(test)]
pub(crate) use mocks::*;
