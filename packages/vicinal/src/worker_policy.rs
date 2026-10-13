use std::num::NonZero;

use many_cpus::SystemHardware;

use crate::{GroupId, SubmissionTarget};

/// Configures worker counts and the locality of submitted tasks.
///
/// Set a policy with [`PoolBuilder::worker_policy`][crate::PoolBuilder::worker_policy].
/// A pool's policy is fixed when it is built. Workers start lazily on the first submission
/// to a processor or memory region and persist until shutdown. Counts are not limited by
/// the number of processors, allowing oversubscription for blocking work.
///
/// Locality requires task code to preserve worker affinity. It does not bind allocations,
/// move data or guarantee cache hits. Policies do not enforce processor-time quotas.
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
    /// Each task is routed to its submitting processor's [`many_cpus::MemoryRegionId`].
    /// Workers may run only on processors in that region that have submitted accepted work
    /// to this pool. The first submission starts the configured workers on its processor;
    /// new submitting processors expand the permitted set without adding workers.
    ///
    /// Workers refresh affinity at task boundaries. A running task may finish with the
    /// previous narrower set and may migrate within that set. Membership is cumulative:
    /// processors are never removed, and there is no cross-region work stealing.
    ///
    /// Submitters using a stable, quota-appropriate processor set keep worker placement
    /// within that set. Changing submitter processors can grow the accumulated set.
    /// This does not guarantee avoidance of throttling from fractional quotas or other work.
    ///
    /// # Example
    ///
    /// ```
    /// use new_zealand::nz;
    /// use vicinal::{Pool, WorkerPolicy};
    /// # use futures::executor::block_on;
    ///
    /// # block_on(async {
    /// // Illustrative concurrency; choose the count for your blocking workload.
    /// let pool = Pool::builder()
    ///     .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(2_u32)))
    ///     .build();
    /// assert_eq!(pool.scheduler().spawn(|| 42).await?, 42);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # }).unwrap();
    /// ```
    #[must_use]
    pub fn fixed_per_memory_region(count: NonZero<u32>) -> Self {
        Self {
            kind: PolicyKind::MemoryRegion,
            count,
        }
    }

    /// Samples only the identity used by this policy, avoiding inconsistent topology samples.
    pub(crate) fn destination(&self, hardware: &SystemHardware) -> SubmissionTarget {
        match self.kind {
            PolicyKind::Processor => {
                let processor = hardware.current_processor_id();
                SubmissionTarget {
                    group: GroupId::Processor(processor),
                    processor,
                }
            }
            PolicyKind::MemoryRegion => {
                hardware.with_current_processor(|processor| SubmissionTarget {
                    group: GroupId::MemoryRegion(processor.memory_region_id()),
                    processor: processor.id(),
                })
            }
        }
    }

    pub(crate) fn is_regional(&self) -> bool {
        matches!(self.kind, PolicyKind::MemoryRegion)
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
