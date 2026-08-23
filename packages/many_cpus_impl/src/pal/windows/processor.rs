use std::fmt::Display;
use std::sync::Arc;

use crate::pal::AbstractProcessor;
use crate::pal::windows::{ProcessorGroupIndex, ProcessorIndexInGroup};
use crate::{EfficiencyClass, MemoryRegionId, ProcessorId, RelativeSpeed};

#[derive(Clone, Debug)]
pub(crate) struct ProcessorImpl {
    pub(crate) group_index: ProcessorGroupIndex,
    pub(crate) index_in_group: ProcessorIndexInGroup,

    // Cumulative index when counting across all groups.
    pub(crate) id: ProcessorId,

    pub(crate) memory_region_id: MemoryRegionId,

    pub(crate) efficiency_class: EfficiencyClass,

    pub(crate) relative_speed: RelativeSpeed,

    pub(crate) model: Option<Arc<str>>,
}

impl ProcessorImpl {
    pub(crate) fn new(
        group_index: ProcessorGroupIndex,
        index_in_group: ProcessorIndexInGroup,
        id: ProcessorId,
        memory_region_id: MemoryRegionId,
        efficiency_class: EfficiencyClass,
        relative_speed: RelativeSpeed,
        model: Option<Arc<str>>,
    ) -> Self {
        Self {
            group_index,
            index_in_group,
            id,
            memory_region_id,
            efficiency_class,
            relative_speed,
            model,
        }
    }
}

impl Display for ProcessorImpl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "processor {} [{}.{}]",
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

    fn relative_speed(&self) -> RelativeSpeed {
        self.relative_speed
    }

    fn model(&self) -> Option<&str> {
        self.model.as_deref()
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
        let processor = ProcessorImpl::new(
            0,
            1,
            2,
            3,
            EfficiencyClass::Performance,
            RelativeSpeed::from_raw(3600),
            Some(Arc::from("Test Model")),
        );

        assert_eq!(processor.id(), 2);
        assert_eq!(processor.memory_region_id(), 3);
        assert_eq!(processor.efficiency_class(), EfficiencyClass::Performance);
        assert_eq!(processor.relative_speed().as_u64(), 3600);
        assert_eq!(processor.model(), Some("Test Model"));
    }

    #[test]
    fn display_shows_processor_id_and_group_info() {
        // group_index=1, index_in_group=3, id=7
        let processor = ProcessorImpl::new(
            1,
            3,
            7,
            0,
            EfficiencyClass::Performance,
            RelativeSpeed::from_raw(1),
            None,
        );

        let display_output = processor.to_string();

        assert!(
            display_output.contains("processor 7"),
            "display should contain processor ID: {display_output}"
        );
        assert!(
            display_output.contains("[1.3]"),
            "display should contain group.index format: {display_output}"
        );
    }

    #[test]
    fn as_ref_returns_self() {
        let processor = ProcessorImpl::new(
            0,
            2,
            5,
            1,
            EfficiencyClass::Efficiency,
            RelativeSpeed::from_raw(1),
            None,
        );

        let processor_ref: &ProcessorImpl = processor.as_ref();

        assert_eq!(processor_ref.id, processor.id);
        assert_eq!(processor_ref.group_index, processor.group_index);
        assert_eq!(processor_ref.index_in_group, processor.index_in_group);
        assert_eq!(processor_ref.model(), None);
    }
}
