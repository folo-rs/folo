//! Process PAL.

mod abstractions;
mod command_line;
mod facade;
#[cfg(not(windows))]
mod unsupported;
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
