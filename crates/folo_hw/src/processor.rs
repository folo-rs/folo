use std::fmt::Display;

use crate::{
    pal::{self, EfficiencyClass, MemoryRegionIndex, ProcessorCommon, ProcessorGlobalIndex},
    ProcessorCore,
};

/// A processor present on the system and available to the current process.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Processor {
    inner: ProcessorCore<pal::Platform>,
}

impl Processor {
    pub(crate) fn new(inner: ProcessorCore<pal::Platform>) -> Self {
        Self { inner }
    }
}

impl Display for Processor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.inner, f)
    }
}

impl ProcessorCommon for Processor {
    fn index(&self) -> ProcessorGlobalIndex {
        self.inner.index()
    }

    fn memory_region(&self) -> MemoryRegionIndex {
        self.inner.memory_region()
    }

    fn efficiency_class(&self) -> EfficiencyClass {
        self.inner.efficiency_class()
    }
}

impl AsRef<pal::Processor> for Processor {
    fn as_ref(&self) -> &pal::Processor {
        self.inner.as_ref()
    }
}

impl AsRef<ProcessorCore<pal::Platform>> for Processor {
    fn as_ref(&self) -> &ProcessorCore<pal::Platform> {
        &self.inner
    }
}

impl From<pal::Processor> for Processor {
    fn from(value: pal::Processor) -> Self {
        Self::new(ProcessorCore::new(value, &pal::Platform))
    }
}

impl From<Processor> for ProcessorCore<pal::Platform> {
    fn from(value: Processor) -> Self {
        *value.as_ref()
    }
}

impl From<Processor> for pal::Processor {
    fn from(value: Processor) -> Self {
        *value.as_ref()
    }
}

impl From<ProcessorCore<pal::Platform>> for Processor {
    fn from(value: ProcessorCore<pal::Platform>) -> Self {
        Self::new(value)
    }
}
