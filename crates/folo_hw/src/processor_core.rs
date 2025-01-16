use std::fmt::Display;

use crate::pal::{EfficiencyClass, MemoryRegionIndex, Platform, Processor, ProcessorGlobalIndex};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ProcessorCore<PAL: Platform> {
    inner: PAL::Processor,
    
    pub(crate) pal: &'static PAL,
}

impl<PAL: Platform> ProcessorCore<PAL> {
    pub(crate) fn index(&self) -> ProcessorGlobalIndex {
        self.inner.index()
    }

    pub(crate) fn memory_region(&self) -> MemoryRegionIndex {
        self.inner.memory_region()
    }

    pub(crate) fn efficiency_class(&self) -> EfficiencyClass {
        self.inner.efficiency_class()
    }
}

impl<PAL: Platform> ProcessorCore<PAL> {
    pub(crate) fn new(inner: PAL::Processor, pal: &'static PAL) -> Self {
        Self { inner, pal }
    }
}

impl<PAL: Platform> Display for ProcessorCore<PAL> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.inner, f)
    }
}

impl<PAL: Platform> AsRef<PAL::Processor> for ProcessorCore<PAL> {
    fn as_ref(&self) -> &PAL::Processor {
        &self.inner
    }
}
