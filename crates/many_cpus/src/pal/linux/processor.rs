use std::fmt::Display;

use crate::{pal::AbstractProcessor, EfficiencyClass, MemoryRegionId, ProcessorId};

/// A processor present on the system and available to the current process.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ProcessorImpl {
    pub(super) id: ProcessorId,
    pub(super) memory_region_id: MemoryRegionId,
    pub(super) efficiency_class: EfficiencyClass,

    pub(super) is_active: bool,
}

impl Display for ProcessorImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "processor {} [node {}]", self.id, self.memory_region_id)
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

impl AsRef<ProcessorImpl> for ProcessorImpl {
    fn as_ref(&self) -> &ProcessorImpl {
        self
    }
}
