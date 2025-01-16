mod bindings;
mod platform;
mod platform_core;
mod processor;

use bindings::*;
pub(crate) use platform::*;
use platform_core::*;
pub(crate) use processor::*;

type ProcessorGroupIndex = u16;
type ProcessorIndexInGroup = u8;
