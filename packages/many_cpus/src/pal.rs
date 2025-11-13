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

// The fallback module is compiled in test mode on all platforms, and as the primary
// implementation on unsupported platforms. However, we only glob-import it when it is
// the primary implementation (i.e. on unsupported platforms). On supported platforms
// in test mode, it must be accessed via the explicit path `fallback::` to avoid ambiguity
// with the platform-specific implementation.
#[cfg(any(test, not(any(target_os = "linux", windows))))]
pub(crate) mod fallback;

#[cfg(not(any(target_os = "linux", windows)))]
pub(crate) use fallback::*;

#[cfg(test)]
mod mocks;
#[cfg(test)]
pub(crate) use mocks::*;
