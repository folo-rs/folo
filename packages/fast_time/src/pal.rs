mod abstractions;
mod facade;

pub(crate) use abstractions::*;
pub(crate) use facade::*;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub(crate) use linux::*;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::*;

#[cfg(test)]
mod mock;
#[cfg(test)]
pub(crate) use mock::*;
