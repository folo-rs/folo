use std::collections::BTreeSet;
use std::iter;
use std::sync::OnceLock;

use many_cpus::{MemoryRegionId, Processor, SystemHardware};

use crate::{GroupId, WorkerGroup, WorkerPolicy};

/// Lazily allocates group storage with indexed lookup and no hot-path topology filtering.
pub(crate) struct GroupRegistry {
    groups: Box<[OnceLock<WorkerGroup>]>,
    /// Region IDs can be sparse; binary search maps only populated regions to compact slots.
    region_ids: Option<Box<[MemoryRegionId]>>,
}

impl GroupRegistry {
    pub(crate) fn new(hardware: &SystemHardware, policy: &WorkerPolicy) -> Self {
        let region_ids = policy.is_regional().then(|| {
            hardware
                .all_processors()
                .iter()
                .map(Processor::memory_region_id)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Box<[_]>>()
        });
        let count = region_ids
            .as_ref()
            .map_or_else(|| hardware.max_processor_count(), |ids| ids.len());
        Self {
            groups: iter::repeat_with(OnceLock::new).take(count).collect(),
            region_ids,
        }
    }

    pub(crate) fn get_or_init(&self, destination: GroupId) -> &WorkerGroup {
        let index = match (&self.region_ids, destination) {
            (None, GroupId::Processor(id)) => id as usize,
            (Some(ids), GroupId::MemoryRegion(id)) => ids
                .binary_search(&id)
                .expect("a submitting region must contain an eligible processor"),
            _ => panic!("the destination must match the pool's policy"),
        };
        self.groups
            .get(index)
            .expect("the policy samples IDs within its hardware index space")
            .get_or_init(WorkerGroup::new)
    }

    pub(crate) fn initialized(&self) -> impl Iterator<Item = &WorkerGroup> {
        self.groups.iter().filter_map(OnceLock::get)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::ptr;

    use many_cpus::fake::{HardwareBuilder, ProcessorBuilder};
    use new_zealand::nz;
    use testing::assert_panics;

    use super::*;

    #[test]
    fn sparse_groups_are_lazy_and_shared() {
        // Nonadjacent region IDs distinguish the index-space bound from populated regions.
        let hardware = SystemHardware::fake(
            HardwareBuilder::new()
                .processor(ProcessorBuilder::new().id(1).memory_region(5))
                .processor(ProcessorBuilder::new().id(3).memory_region(2))
                .processor(ProcessorBuilder::new().id(4).memory_region(5)),
        );
        let policy = WorkerPolicy::fixed_per_memory_region(nz!(1_u32));
        let registry = GroupRegistry::new(&hardware, &policy);
        assert_eq!(registry.groups.len(), 2);
        assert_eq!(registry.initialized().count(), 0);
        assert_panics(|| registry.get_or_init(GroupId::MemoryRegion(4)));
        let first = registry.get_or_init(GroupId::MemoryRegion(2));
        assert!(ptr::eq(
            first,
            registry.get_or_init(GroupId::MemoryRegion(2))
        ));
        assert!(!ptr::eq(
            first,
            registry.get_or_init(GroupId::MemoryRegion(5))
        ));
        assert_eq!(registry.initialized().count(), 2);
    }
}
