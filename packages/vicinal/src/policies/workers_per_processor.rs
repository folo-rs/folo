use std::any::type_name;
use std::fmt::{Debug, Formatter, Result as FmtResult};
use std::num::NonZero;

#[cfg(test)]
use many_cpus::{ProcessorId, ProcessorSet};

#[cfg(test)]
use crate::WorkerGroup;
use crate::{BuiltInWorkerPolicy, WorkerGroupId, WorkerGroups, WorkerPolicy, WorkerPolicyContext};

/// Creates a fixed worker count on each submitting processor.
///
/// Each task is routed to the logical processor sampled once during submission.
#[derive(Clone)]
pub struct WorkersPerProcessor {
    policy: BuiltInWorkerPolicy,
}

impl WorkersPerProcessor {
    /// Creates a processor-local policy with the given worker count.
    #[must_use]
    #[inline]
    pub fn new(count: NonZero<u32>) -> Self {
        Self {
            policy: BuiltInWorkerPolicy::per_processor(count),
        }
    }
}

impl WorkerPolicy for WorkersPerProcessor {
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

impl Debug for WorkersPerProcessor {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct(type_name::<Self>())
            .field("workers", &self.policy.worker_count())
            .finish()
    }
}
