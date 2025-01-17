use std::fmt::Display;

use derive_more::derive::AsRef;

use crate::{pal, ProcessorCore};

/// A processor present on the system and available to the current process.
#[derive(AsRef, Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Processor {
    #[as_ref(ProcessorCore<pal::PlatformImpl>, pal::ProcessorImpl)]
    inner: ProcessorCore<pal::PlatformImpl>,
}

impl Processor {
    pub(crate) fn new(inner: ProcessorCore<pal::PlatformImpl>) -> Self {
        Self { inner }
    }
}

impl Display for Processor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.inner, f)
    }
}

impl From<ProcessorCore<pal::PlatformImpl>> for Processor {
    fn from(value: ProcessorCore<pal::PlatformImpl>) -> Self {
        Self::new(value)
    }
}
