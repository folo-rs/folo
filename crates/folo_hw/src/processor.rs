use std::fmt::Display;

use crate::{pal, ProcessorCore};

/// A processor present on the system and available to the current process.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Processor {
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

impl AsRef<pal::ProcessorImpl> for Processor {
    fn as_ref(&self) -> &pal::ProcessorImpl {
        self.inner.as_ref()
    }
}

impl AsRef<ProcessorCore<pal::PlatformImpl>> for Processor {
    fn as_ref(&self) -> &ProcessorCore<pal::PlatformImpl> {
        &self.inner
    }
}

impl From<pal::ProcessorImpl> for Processor {
    fn from(value: pal::ProcessorImpl) -> Self {
        Self::new(ProcessorCore::new(value, &pal::CURRENT))
    }
}

impl From<Processor> for ProcessorCore<pal::PlatformImpl> {
    fn from(value: Processor) -> Self {
        *value.as_ref()
    }
}

impl From<Processor> for pal::ProcessorImpl {
    fn from(value: Processor) -> Self {
        *value.as_ref()
    }
}

impl From<ProcessorCore<pal::PlatformImpl>> for Processor {
    fn from(value: ProcessorCore<pal::PlatformImpl>) -> Self {
        Self::new(value)
    }
}
