use std::fmt::Display;
use std::hash::{Hash, Hasher};
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

// A processor is uniquely identified by its `id`. The other fields are descriptive metadata that
// never differs between two handles to the same processor, so equality, hashing, and ordering are
// all keyed on `id` alone. Keeping them in agreement upholds the `Ord`/`Eq` contract:
// `cmp(a, b) == Equal` exactly when `a == b`.
impl PartialEq for ProcessorImpl {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for ProcessorImpl {}

impl Hash for ProcessorImpl {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
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

        let processor2 = ProcessorImpl::new(
            0,
            1,
            2,
            3,
            EfficiencyClass::Performance,
            RelativeSpeed::from_raw(3600),
            Some(Arc::from("Test Model")),
        );
        assert_eq!(processor, processor2);

        let processor3 = ProcessorImpl::new(
            0,
            1,
            4,
            3,
            EfficiencyClass::Performance,
            RelativeSpeed::from_raw(3600),
            Some(Arc::from("Test Model")),
        );
        assert_ne!(processor, processor3);
        assert!(processor < processor3);
        assert!(processor3 > processor);
    }

    #[test]
    fn equal_ids_compare_equal_regardless_of_metadata() {
        use std::hash::{DefaultHasher, Hash, Hasher};

        // Two handles to the same processor id are equal and order as Equal even when their
        // descriptive metadata differs, keeping the Ord and Eq/Hash impls mutually consistent.
        let a = ProcessorImpl::new(
            0,
            1,
            7,
            3,
            EfficiencyClass::Performance,
            RelativeSpeed::from_raw(3600),
            Some(Arc::from("Model A")),
        );
        let b = ProcessorImpl::new(
            2,
            4,
            7,
            9,
            EfficiencyClass::Efficiency,
            RelativeSpeed::SYNTHETIC,
            None,
        );

        assert_eq!(a, b);
        assert_eq!(a.cmp(&b), std::cmp::Ordering::Equal);

        let mut hash_a = DefaultHasher::new();
        a.hash(&mut hash_a);
        let mut hash_b = DefaultHasher::new();
        b.hash(&mut hash_b);
        assert_eq!(hash_a.finish(), hash_b.finish());

        // Hashing is keyed on `id` alone, so a processor hashes identically to its bare `id`.
        // This also confirms the hash actually incorporates the id rather than being a no-op.
        let mut id_hash = DefaultHasher::new();
        a.id.hash(&mut id_hash);
        assert_eq!(hash_a.finish(), id_hash.finish());
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
            RelativeSpeed::SYNTHETIC,
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
            RelativeSpeed::SYNTHETIC,
            None,
        );

        let processor_ref: &ProcessorImpl = processor.as_ref();

        assert_eq!(processor_ref.id, processor.id);
        assert_eq!(processor_ref.group_index, processor.group_index);
        assert_eq!(processor_ref.index_in_group, processor.index_in_group);
        assert_eq!(processor_ref.model(), None);
    }
}
