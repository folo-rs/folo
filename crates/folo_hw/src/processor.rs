use std::fmt::Display;

use derive_more::derive::AsRef;

use crate::{pal, ProcessorCore};

/// A processor present on the system and available to the current process.
#[derive(AsRef, Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Processor {
    #[as_ref(ProcessorCore<pal::CurrentPlatform>, pal::ProcessorImpl)]
    core: ProcessorCore<pal::CurrentPlatform>,
}

impl Processor {
    pub(crate) fn new(inner: ProcessorCore<pal::CurrentPlatform>) -> Self {
        Self { core: inner }
    }
}

impl Display for Processor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.core, f)
    }
}

impl From<ProcessorCore<pal::CurrentPlatform>> for Processor {
    fn from(value: ProcessorCore<pal::CurrentPlatform>) -> Self {
        Self::new(value)
    }
}
