use std::fmt::Display;

use derive_more::derive::AsRef;

use crate::{pal, ProcessorCore};

/// A processor present on the system and available to the current process.
#[derive(AsRef, Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Processor {
    #[as_ref(ProcessorCore<pal::BuildTargetPlatform>, pal::ProcessorImpl)]
    core: ProcessorCore<pal::BuildTargetPlatform>,
}

impl Processor {
    pub(crate) fn new(inner: ProcessorCore<pal::BuildTargetPlatform>) -> Self {
        Self { core: inner }
    }
}

impl Display for Processor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.core, f)
    }
}

impl From<ProcessorCore<pal::BuildTargetPlatform>> for Processor {
    fn from(value: ProcessorCore<pal::BuildTargetPlatform>) -> Self {
        Self::new(value)
    }
}
