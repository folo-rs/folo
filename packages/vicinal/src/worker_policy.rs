use std::num::NonZero;

use many_cpus::SystemHardware;

use crate::GroupId;

/// Configures worker counts and the locality of submitted tasks.
///
/// Set a policy with [`PoolBuilder::worker_policy`][crate::PoolBuilder::worker_policy].
/// A pool's policy is fixed when it is built. Workers start lazily on the first submission
/// to a processor or memory region and persist until shutdown. Counts are not limited by
/// the number of processors, allowing oversubscription for blocking work.
///
/// Locality requires task code to preserve worker affinity. It does not bind allocations,
/// move data, guarantee cache hits or avoid processor-time quota throttling.
#[derive(Clone, Debug)]
pub struct WorkerPolicy {
    kind: PolicyKind,
    count: NonZero<u32>,
}

impl WorkerPolicy {
    /// Uses a fixed number of workers pinned to each submitting processor.
    ///
    /// Each task is routed to the logical processor sampled during its submission.
    #[must_use]
    pub fn fixed_per_processor(count: NonZero<u32>) -> Self {
        Self {
            kind: PolicyKind::Processor,
            count,
        }
    }

    /// Uses a fixed number of workers shared by each submitting memory region.
    ///
    /// Each task is routed to the [`many_cpus::MemoryRegionId`] sampled during submission.
    /// Workers may run on any eligible processor in that region, including migrating
    /// during one task. There is no cross-region work stealing.
    ///
    /// # Example
    ///
    /// ```
    /// use new_zealand::nz;
    /// use vicinal::{Pool, WorkerPolicy};
    /// # use futures::executor::block_on;
    ///
    /// # block_on(async {
    /// let pool = Pool::builder()
    ///     .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(2_u32)))
    ///     .build();
    /// assert_eq!(pool.scheduler().spawn(|| 42).await, 42);
    /// # });
    /// ```
    #[must_use]
    pub fn fixed_per_memory_region(count: NonZero<u32>) -> Self {
        Self {
            kind: PolicyKind::MemoryRegion,
            count,
        }
    }

    /// Samples only the identity used by this policy, avoiding inconsistent topology samples.
    pub(crate) fn destination(&self, hardware: &SystemHardware) -> GroupId {
        match self.kind {
            PolicyKind::Processor => GroupId::Processor(hardware.current_processor_id()),
            PolicyKind::MemoryRegion => GroupId::MemoryRegion(hardware.current_memory_region_id()),
        }
    }

    /// Sizes the sparse index space, not the number of populated groups.
    pub(crate) fn capacity(&self, hardware: &SystemHardware) -> usize {
        match self.kind {
            PolicyKind::Processor => hardware.max_processor_count(),
            PolicyKind::MemoryRegion => hardware.max_memory_region_count(),
        }
    }

    pub(crate) fn count(&self) -> NonZero<u32> {
        self.count
    }
}

/// Keeps policy variants private so extensions need not change caller matches.
#[derive(Clone, Debug)]
enum PolicyKind {
    Processor,
    MemoryRegion,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(WorkerPolicy: Clone, Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(WorkerPolicy: Copy);
}
