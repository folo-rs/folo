use std::any::type_name;
use std::fmt;
use std::num::NonZero;

#[cfg(test)]
use many_cpus::{ProcessorId, ProcessorSet};

#[cfg(test)]
use crate::WorkerGroup;
use crate::{BuiltInWorkerPolicy, WorkerGroupId, WorkerGroups, WorkerPolicy, WorkerPolicyContext};

/// Creates a fixed worker count shared across the system.
///
/// The worker mask starts with eager processors or the first submitter and expands
/// cumulatively across memory-region boundaries.
#[derive(Clone)]
pub struct WorkersPerSystem {
    policy: BuiltInWorkerPolicy,
}

impl WorkersPerSystem {
    /// Creates a system-wide policy with the given total worker count.
    #[must_use]
    #[inline]
    pub fn new(count: NonZero<u32>) -> Self {
        Self {
            policy: BuiltInWorkerPolicy::per_system(count),
        }
    }
}

impl WorkerPolicy for WorkersPerSystem {
    #[inline]
    fn prepare(&mut self, context: WorkerPolicyContext<'_>) {
        WorkerPolicy::prepare(&mut self.policy, context);
    }

    #[inline]
    fn select_group(&self, groups: &WorkerGroups) -> WorkerGroupId {
        WorkerPolicy::select_group(&self.policy, groups)
    }

    #[cfg(test)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn membership_snapshot(&self) -> Vec<(ProcessorId, bool)> {
        WorkerPolicy::membership_snapshot(&self.policy)
    }

    #[cfg(test)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn install_test_group(
        &self,
        groups: &WorkerGroups,
        group: &WorkerGroup,
        affinity: &ProcessorSet,
    ) {
        WorkerPolicy::install_test_group(&self.policy, groups, group, affinity);
    }
}

impl fmt::Debug for WorkersPerSystem {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(type_name::<Self>())
    }
}
