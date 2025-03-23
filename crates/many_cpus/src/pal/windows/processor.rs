use std::fmt::Display;

use crate::{
    EfficiencyClass, MemoryRegionId, ProcessorId,
    pal::{
        AbstractProcessor,
        windows::{ProcessorGroupIndex, ProcessorIndexInGroup},
    },
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ProcessorImpl {
    pub(super) group_index: ProcessorGroupIndex,
    pub(super) index_in_group: ProcessorIndexInGroup,

    // Cumulative index when counting across all groups.
    pub(super) id: ProcessorId,

    pub(super) memory_region_id: MemoryRegionId,

    pub(super) efficiency_class: EfficiencyClass,
}

impl ProcessorImpl {
    pub(super) fn new(
        group_index: ProcessorGroupIndex,
        index_in_group: ProcessorIndexInGroup,
        id: ProcessorId,
        memory_region_id: MemoryRegionId,
        efficiency_class: EfficiencyClass,
    ) -> Self {
        Self {
            group_index,
            index_in_group,
            id,
            memory_region_id,
            efficiency_class,
        }
    }
}

impl Display for ProcessorImpl {
    #[cfg_attr(test, mutants::skip)] // There no API contract to test here.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "processor {} [{}-{}]",
            self.id, self.group_index, self.index_in_group
        )
    }
}

impl AbstractProcessor for ProcessorImpl {
    fn id(&self) -> ProcessorId {
        self.id
    }

    fn memory_region_id(&self) -> MemoryRegionId {
        self.memory_region_id
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
        self.id.cmp(&other.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_test() {
        let processor = ProcessorImpl::new(0, 1, 2, 3, EfficiencyClass::Performance);

        assert_eq!(processor.id(), 2);
        assert_eq!(processor.memory_region_id(), 3);
        assert_eq!(processor.efficiency_class(), EfficiencyClass::Performance);

        let processor2 = ProcessorImpl::new(0, 1, 2, 3, EfficiencyClass::Performance);
        assert_eq!(processor, processor2);

        let processor3 = ProcessorImpl::new(0, 1, 4, 3, EfficiencyClass::Performance);
        assert_ne!(processor, processor3);
        assert!(processor < processor3);
        assert!(processor3 > processor);
    }
}
