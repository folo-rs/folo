use std::{
    fmt::Display,
    hash::{Hash, Hasher},
};

use derive_more::derive::AsRef;

use crate::pal::{EfficiencyClass, MemoryRegionIndex, Platform, Processor, ProcessorGlobalIndex};

#[derive(AsRef, Debug)]
pub(crate) struct ProcessorCore<PAL: Platform> {
    #[as_ref]
    inner: PAL::Processor,

    pub(crate) pal: &'static PAL,
}

impl<PAL: Platform> ProcessorCore<PAL> {
    pub(crate) fn new(inner: PAL::Processor, pal: &'static PAL) -> Self {
        Self { inner, pal }
    }

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

impl<PAL: Platform> Copy for ProcessorCore<PAL> {}
impl<PAL: Platform> Clone for ProcessorCore<PAL> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<PAL: Platform> PartialEq for ProcessorCore<PAL> {
    fn eq(&self, other: &Self) -> bool {
        self.inner == other.inner
    }
}
impl<PAL: Platform> Eq for ProcessorCore<PAL> {}

impl<PAL: Platform> Hash for ProcessorCore<PAL> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inner.hash(state)
    }
}

impl<PAL: Platform> Display for ProcessorCore<PAL> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(&self.inner, f)
    }
}
