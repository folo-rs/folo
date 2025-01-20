mod bindings;
mod native_buffer;
mod platform;
mod platform_core;
mod processor;

use bindings::*;
#[cfg(test)] // For now, later also in release mode
use native_buffer::*;
pub(crate) use platform::*;
use platform_core::*;
pub(crate) use processor::*;

type ProcessorGroupIndex = u16;
type ProcessorIndexInGroup = u8;
