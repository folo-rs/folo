use std::fmt::Display;

use crate::pal::{EfficiencyClass, MemoryRegionIndex, Processor, ProcessorGlobalIndex};

/// A processor present on the system and available to the current process.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ProcessorImpl {
    pub(super) index: ProcessorGlobalIndex,
    pub(super) memory_region: MemoryRegionIndex,
    pub(super) efficiency_class: EfficiencyClass,
}

impl Display for ProcessorImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "processor {} [node {}]", self.index, self.memory_region)
    }
}

impl Processor for ProcessorImpl {
    fn index(&self) -> ProcessorGlobalIndex {
        self.index
    }

    fn memory_region(&self) -> MemoryRegionIndex {
        self.memory_region
    }

    fn efficiency_class(&self) -> EfficiencyClass {
        self.efficiency_class
    }
}
