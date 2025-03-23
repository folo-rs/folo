mod bindings;
mod platform;
mod processor;

use bindings::*;
pub(crate) use platform::*;
pub(crate) use processor::*;

type ProcessorGroupIndex = u16;
type ProcessorIndexInGroup = u8;
