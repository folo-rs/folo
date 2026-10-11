use std::iter;
use std::sync::OnceLock;

use many_cpus::SystemHardware;

use crate::{GroupId, WorkerGroup, WorkerPolicy};

/// Lazily allocates group storage with indexed lookup and no hot-path topology filtering.
pub(crate) struct GroupRegistry {
    groups: Box<[OnceLock<WorkerGroup>]>,
}

impl GroupRegistry {
    pub(crate) fn new(hardware: &SystemHardware, policy: &WorkerPolicy) -> Self {
        Self {
            groups: iter::repeat_with(OnceLock::new)
                .take(policy.capacity(hardware))
                .collect(),
        }
    }

    pub(crate) fn get_or_init(&self, destination: GroupId) -> &WorkerGroup {
        self.groups
            .get(destination.index())
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

    use super::*;

    #[test]
    fn sparse_groups_are_lazy_and_shared() {
        // Nonadjacent region IDs distinguish the index-space bound from populated regions.
        let hardware = SystemHardware::fake(
            HardwareBuilder::new()
                .processor(ProcessorBuilder::new().id(1).memory_region(2))
                .processor(ProcessorBuilder::new().id(3).memory_region(5)),
        );
        let policy = WorkerPolicy::fixed_per_memory_region(nz!(1_u32));
        let registry = GroupRegistry::new(&hardware, &policy);
        assert_eq!(registry.groups.len(), 6);
        assert_eq!(registry.initialized().count(), 0);
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
