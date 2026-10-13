use std::collections::BTreeSet;
use std::sync::OnceLock;
use std::{iter, slice};

use many_cpus::{MemoryRegionId, Processor, SystemHardware};

use crate::{GroupId, PolicyKind, WorkerGroup, WorkerPolicy};

/// Lazily allocates group storage with indexed lookup and no hot-path topology filtering.
pub(crate) enum GroupRegistry {
    Processor(Box<[OnceLock<WorkerGroup>]>),
    /// Region IDs can be sparse; binary search maps only populated regions to compact slots.
    MemoryRegion {
        ids: Box<[MemoryRegionId]>,
        groups: Box<[OnceLock<WorkerGroup>]>,
    },
    System(Box<OnceLock<WorkerGroup>>),
}

impl GroupRegistry {
    pub(crate) fn new(hardware: &SystemHardware, policy: &WorkerPolicy) -> Self {
        match policy.kind() {
            PolicyKind::Processor => Self::Processor(
                iter::repeat_with(OnceLock::new)
                    .take(hardware.max_processor_count())
                    .collect(),
            ),
            PolicyKind::MemoryRegion => {
                let ids = hardware
                    .all_processors()
                    .iter()
                    .map(Processor::memory_region_id)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Box<[_]>>();
                Self::MemoryRegion {
                    groups: iter::repeat_with(OnceLock::new).take(ids.len()).collect(),
                    ids,
                }
            }
            PolicyKind::System => Self::System(Box::default()),
        }
    }

    pub(crate) fn get_or_init(&self, destination: GroupId) -> &WorkerGroup {
        let group = match (self, destination) {
            (Self::Processor(groups), GroupId::Processor(id)) => groups.get(id as usize),
            (Self::MemoryRegion { ids, groups }, GroupId::MemoryRegion(id)) => groups.get(
                ids.binary_search(&id)
                    .expect("a submitting region must contain an eligible processor"),
            ),
            (Self::System(group), GroupId::System) => Some(group.as_ref()),
            _ => panic!("the destination must match the pool's policy"),
        };
        group
            .expect("the policy samples IDs within its hardware index space")
            .get_or_init(WorkerGroup::new)
    }

    pub(crate) fn initialized(&self) -> impl Iterator<Item = &WorkerGroup> {
        let groups = match self {
            Self::Processor(groups) | Self::MemoryRegion { groups, .. } => groups.as_ref(),
            Self::System(group) => slice::from_ref(group.as_ref()),
        };
        groups.iter().filter_map(OnceLock::get)
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
        let GroupRegistry::MemoryRegion { groups, .. } = &registry else {
            panic!("regional policy must use regional indexing");
        };
        assert_eq!(groups.len(), 2);
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

    /// The system group ignores topology size and remains isolated from other pools.
    #[test]
    fn system_group_is_lazy_shared_and_pool_local() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(2), nz!(2)));
        let policy = WorkerPolicy::fixed_per_system(nz!(1));
        let registry = GroupRegistry::new(&hardware, &policy);
        assert_eq!(registry.initialized().count(), 0);
        let first = registry.get_or_init(GroupId::System);
        assert!(ptr::eq(first, registry.get_or_init(GroupId::System)));
        assert_eq!(registry.initialized().count(), 1);
        assert_panics(|| registry.get_or_init(GroupId::Processor(0)));
        assert_panics(|| registry.get_or_init(GroupId::MemoryRegion(0)));
        let independent = GroupRegistry::new(&hardware, &policy);
        assert!(!ptr::eq(first, independent.get_or_init(GroupId::System)));
    }
}
