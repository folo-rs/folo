use std::fmt::Display;

use crate::pal::{AbstractProcessor, EfficiencyClass, MemoryRegionIndex, ProcessorGlobalIndex};

/// A processor present on the system and available to the current process.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ProcessorImpl {
    pub(super) index: ProcessorGlobalIndex,
    pub(super) memory_region_index: MemoryRegionIndex,
    pub(super) efficiency_class: EfficiencyClass,
}

impl Display for ProcessorImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "processor {} [node {}]",
            self.index, self.memory_region_index
        )
    }
}

impl AbstractProcessor for ProcessorImpl {
    fn index(&self) -> ProcessorGlobalIndex {
        self.index
    }

    fn memory_region(&self) -> MemoryRegionIndex {
        self.memory_region_index
    }

    fn efficiency_class(&self) -> EfficiencyClass {
        self.efficiency_class
    }
}

impl PartialOrd for ProcessorImpl {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ProcessorImpl {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.index.cmp(&other.index)
    }
}

impl AsRef<ProcessorImpl> for ProcessorImpl {
    fn as_ref(&self) -> &ProcessorImpl {
        self
    }
}