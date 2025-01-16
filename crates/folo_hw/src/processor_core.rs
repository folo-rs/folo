use std::fmt::Display;

use crate::pal::{
    self, EfficiencyClass, MemoryRegionIndex, Platform, Processor, ProcessorGlobalIndex,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ProcessorCore<PAL>
where
    PAL: Platform,
{
    inner: PAL::Processor,
    pub(crate) pal: &'static PAL,
}

impl<PAL> ProcessorCore<PAL>
where
    PAL: Platform,
{
    pub(crate) fn new(inner: PAL::Processor, pal: &'static PAL) -> Self {
        Self { inner, pal }
    }
}

impl<PAL> Display for ProcessorCore<PAL>
where
    PAL: Platform,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.inner, f)
    }
}

impl<PAL> Processor for ProcessorCore<PAL>
where
    PAL: Platform,
{
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

impl<PAL> AsRef<PAL::Processor> for ProcessorCore<PAL>
where
    PAL: Platform,
{
    fn as_ref(&self) -> &PAL::Processor {
        &self.inner
    }
}

impl From<ProcessorCore<pal::PlatformImpl>> for pal::ProcessorImpl {
    fn from(value: ProcessorCore<pal::PlatformImpl>) -> Self {
        *value.as_ref()
    }
}
