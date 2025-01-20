mod bindings;
mod native_buffer;
mod platform;
mod processor;

use bindings::*;
#[cfg(test)] // For now, later also in release mode
use native_buffer::*;
pub(crate) use platform::*;
pub(crate) use processor::*;

type ProcessorGroupIndex = u16;
type ProcessorIndexInGroup = u8;
