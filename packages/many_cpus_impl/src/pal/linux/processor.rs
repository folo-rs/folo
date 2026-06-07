use std::fmt::Display;

use crate::pal::AbstractProcessor;
use crate::{EfficiencyClass, MemoryRegionId, ProcessorId};

/// A processor present on the system and available to the current process.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ProcessorImpl {
    pub(crate) id: ProcessorId,
    pub(crate) memory_region_id: MemoryRegionId,
    pub(crate) efficiency_class: EfficiencyClass,

    pub(crate) is_active: bool,
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

impl AsRef<Self> for ProcessorImpl {
    fn as_ref(&self) -> &Self {
        self
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn smoke_test() {
        let processor = ProcessorImpl {
            id: 2,
            memory_region_id: 3,
            efficiency_class: EfficiencyClass::Performance,
            is_active: true,
        };

        assert_eq!(processor.id(), 2);
        assert_eq!(processor.memory_region_id(), 3);
        assert_eq!(processor.efficiency_class(), EfficiencyClass::Performance);

        let processor2 = ProcessorImpl {
            id: 2,
            memory_region_id: 3,
            efficiency_class: EfficiencyClass::Performance,
            is_active: true,
        };

        assert_eq!(processor, processor2);

        let processor3 = ProcessorImpl {
            id: 4,
            memory_region_id: 3,
            efficiency_class: EfficiencyClass::Performance,
            is_active: true,
        };

        assert_ne!(processor, processor3);
        assert!(processor < processor3);
        assert!(processor3 > processor);
    }

    #[test]
    fn display_shows_processor_id_and_node() {
        let processor = ProcessorImpl {
            id: 5,
            memory_region_id: 2,
            efficiency_class: EfficiencyClass::Efficiency,
            is_active: true,
        };

        let display_output = processor.to_string();

        assert!(
            display_output.contains("processor 5"),
            "display should contain processor ID: {display_output}"
        );
        assert!(
            display_output.contains("node 2"),
            "display should contain memory region (node): {display_output}"
        );
    }

    #[test]
    fn as_ref_returns_self() {
        let processor = ProcessorImpl {
            id: 7,
            memory_region_id: 1,
            efficiency_class: EfficiencyClass::Performance,
            is_active: true,
        };

        let processor_ref: &ProcessorImpl = processor.as_ref();

        assert_eq!(processor_ref.id, processor.id);
        assert_eq!(processor_ref.memory_region_id, processor.memory_region_id);
    }
}
