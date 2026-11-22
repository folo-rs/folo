use std::any::type_name;
use std::fmt;
use std::num::NonZero;

#[cfg(test)]
use many_cpus::{ProcessorId, ProcessorSet};

#[cfg(test)]
use crate::WorkerGroup;
use crate::{BuiltInWorkerPolicy, WorkerGroupId, WorkerGroups, WorkerPolicy, WorkerPolicyContext};

/// Creates a fixed worker count for each submitting memory region.
///
/// The region's affinity starts with its eager processors or first submitter and expands
/// cumulatively as processors in that region submit accepted work.
#[derive(Clone)]
pub struct WorkersPerMemoryRegion {
    policy: BuiltInWorkerPolicy,
}

impl WorkersPerMemoryRegion {
    /// Creates a memory-region-local policy with the given worker count.
    #[must_use]
    #[inline]
    pub fn new(count: NonZero<u32>) -> Self {
        Self {
            policy: BuiltInWorkerPolicy::per_memory_region(count),
        }
    }
}

impl WorkerPolicy for WorkersPerMemoryRegion {
    #[inline]
    fn prepare(&mut self, context: WorkerPolicyContext<'_>) {
        WorkerPolicy::prepare(&mut self.policy, context);
    }

    #[inline]
    fn select_group(&self, groups: &WorkerGroups) -> WorkerGroupId {
        WorkerPolicy::select_group(&self.policy, groups)
    }

    #[cfg(test)]
    fn count(&self) -> NonZero<u32> {
        WorkerPolicy::count(&self.policy)
    }

    #[cfg(test)]
    fn membership_snapshot(&self) -> Vec<(ProcessorId, bool)> {
        WorkerPolicy::membership_snapshot(&self.policy)
    }

    #[cfg(test)]
    fn install_test_group(
        &self,
        groups: &WorkerGroups,
        group: &WorkerGroup,
        affinity: &ProcessorSet,
    ) {
        WorkerPolicy::install_test_group(&self.policy, groups, group, affinity);
    }
}

impl fmt::Debug for WorkersPerMemoryRegion {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(type_name::<Self>())
    }
}
