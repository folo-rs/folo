use std::fmt::Display;
use std::sync::Arc;

use crate::pal::AbstractProcessor;
use crate::{EfficiencyClass, MemoryRegionId, ProcessorId, RelativeSpeed};

/// A processor present on the system and available to the current process.
#[derive(Clone, Debug)]
pub(crate) struct ProcessorImpl {
    pub(crate) id: ProcessorId,
    pub(crate) memory_region_id: MemoryRegionId,
    pub(crate) efficiency_class: EfficiencyClass,
    pub(crate) relative_speed: RelativeSpeed,

    /// Model from the `model name` field of `/proc/cpuinfo`, `None` when absent.
    pub(crate) model: Option<Arc<str>>,

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

    fn relative_speed(&self) -> RelativeSpeed {
        self.relative_speed
    }

    fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }
}

// Linux returns its processor list sorted by `id` (see `platform.rs`), so `ProcessorImpl` orders
// by `id`. The other fields are descriptive metadata that never differs between two handles to the
// same processor, so equality and ordering are both keyed on `id` alone, keeping them in agreement:
// `cmp(a, b) == Equal` exactly when `a == b`.
impl PartialEq for ProcessorImpl {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for ProcessorImpl {}

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
            relative_speed: RelativeSpeed::from_raw(4890),
            model: Some(Arc::from("Test CPU 3000")),
            is_active: true,
        };

        assert_eq!(processor.id(), 2);
        assert_eq!(processor.memory_region_id(), 3);
        assert_eq!(processor.efficiency_class(), EfficiencyClass::Performance);
        assert_eq!(processor.relative_speed().as_u64(), 4890);
        assert_eq!(processor.model(), Some("Test CPU 3000"));

        let processor2 = ProcessorImpl {
            id: 2,
            memory_region_id: 3,
            efficiency_class: EfficiencyClass::Performance,
            relative_speed: RelativeSpeed::from_raw(4890),
            model: Some(Arc::from("Test CPU 3000")),
            is_active: true,
        };

        assert_eq!(processor, processor2);

        let processor3 = ProcessorImpl {
            id: 4,
            memory_region_id: 3,
            efficiency_class: EfficiencyClass::Performance,
            relative_speed: RelativeSpeed::from_raw(4890),
            model: None,
            is_active: true,
        };

        assert_ne!(processor, processor3);
        assert_eq!(processor3.model(), None);
        assert!(processor < processor3);
        assert!(processor3 > processor);
    }

    #[test]
    fn equal_ids_compare_equal_regardless_of_metadata() {
        // Two handles to the same processor id are equal and order as Equal even when their
        // descriptive metadata differs, keeping the Eq and Ord impls mutually consistent.
        let a = ProcessorImpl {
            id: 7,
            memory_region_id: 3,
            efficiency_class: EfficiencyClass::Performance,
            relative_speed: RelativeSpeed::from_raw(3600),
            model: Some(Arc::from("Model A")),
            is_active: true,
        };
        let b = ProcessorImpl {
            id: 7,
            memory_region_id: 9,
            efficiency_class: EfficiencyClass::Efficiency,
            relative_speed: RelativeSpeed::from_raw(1),
            model: None,
            is_active: false,
        };

        assert_eq!(a, b);
        assert_eq!(a.cmp(&b), std::cmp::Ordering::Equal);
    }

    #[test]
    fn display_shows_processor_id_and_node() {
        let processor = ProcessorImpl {
            id: 5,
            memory_region_id: 2,
            efficiency_class: EfficiencyClass::Efficiency,
            relative_speed: RelativeSpeed::from_raw(2400),
            model: None,
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
            relative_speed: RelativeSpeed::from_raw(3600),
            model: None,
            is_active: true,
        };

        let processor_ref: &ProcessorImpl = processor.as_ref();

        assert_eq!(processor_ref.id, processor.id);
        assert_eq!(processor_ref.memory_region_id, processor.memory_region_id);
    }
}
