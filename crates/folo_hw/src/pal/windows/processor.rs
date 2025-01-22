use std::fmt::Display;

use crate::pal::{
    windows::{ProcessorGroupIndex, ProcessorIndexInGroup},
    AbstractProcessor, EfficiencyClass, MemoryRegionIndex, ProcessorGlobalIndex,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ProcessorImpl {
    pub(super) group_index: ProcessorGroupIndex,
    pub(super) index_in_group: ProcessorIndexInGroup,

    // Cumulative index when counting across all groups.
    pub(super) global_index: ProcessorGlobalIndex,

    pub(super) memory_region_index: MemoryRegionIndex,

    pub(super) efficiency_class: EfficiencyClass,
}

impl ProcessorImpl {
    pub(super) fn new(
        group_index: ProcessorGroupIndex,
        index_in_group: ProcessorIndexInGroup,
        global_index: ProcessorGlobalIndex,
        memory_region_index: MemoryRegionIndex,
        efficiency_class: EfficiencyClass,
    ) -> Self {
        Self {
            group_index,
            index_in_group,
            global_index,
            memory_region_index,
            efficiency_class,
        }
    }
}

impl Display for ProcessorImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "processor {} [{}-{}]",
            self.global_index, self.group_index, self.index_in_group
        )
    }
}

impl AbstractProcessor for ProcessorImpl {
    fn index(&self) -> ProcessorGlobalIndex {
        self.global_index
    }

    fn memory_region(&self) -> MemoryRegionIndex {
        self.memory_region_index
    }

    fn efficiency_class(&self) -> EfficiencyClass {
        self.efficiency_class
    }
}

impl AsRef<ProcessorImpl> for ProcessorImpl {
    fn as_ref(&self) -> &ProcessorImpl {
        self
    }
}

impl PartialOrd for ProcessorImpl {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ProcessorImpl {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.global_index.cmp(&other.global_index)
    }
}
