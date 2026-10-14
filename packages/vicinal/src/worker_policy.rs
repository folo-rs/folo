use std::num::NonZero;

use many_cpus::SystemHardware;

use crate::{GroupId, SubmissionTarget};

/// Configures worker counts and the locality of submitted tasks.
///
/// Set a policy with [`PoolBuilder::worker_policy`][crate::PoolBuilder::worker_policy].
/// A pool's policy is fixed when it is built. Workers start on the first submission
/// to a destination, or during construction with [`PoolBuilder::eager`][crate::PoolBuilder::eager],
/// and persist until shutdown. Counts are not limited by the number of processors, allowing
/// oversubscription for blocking work.
///
/// Locality requires task code to preserve worker affinity. It does not bind allocations,
/// move data or guarantee cache hits. Policies do not enforce processor-time quotas.
#[derive(Clone, Debug)]
pub struct WorkerPolicy {
    kind: PolicyKind,
    count: NonZero<u32>,
}

impl WorkerPolicy {
    /// Uses a fixed number of workers pinned to each active processor.
    ///
    /// Each task is routed to the logical processor sampled during its submission.
    #[must_use]
    pub fn per_processor(count: NonZero<u32>) -> Self {
        Self {
            kind: PolicyKind::Processor,
            count,
        }
    }

    /// Uses a fixed worker count per memory region.
    ///
    /// Each task is routed to its submitting processor's [`many_cpus::MemoryRegionId`].
    /// Workers may run only on processors in that region selected by
    /// [`PoolBuilder::eager`][crate::PoolBuilder::eager] or that have submitted accepted work.
    /// Without eager selection, the first submission starts the workers on its processor.
    /// New submitting processors expand the permitted set without adding workers.
    ///
    /// Workers refresh affinity at task boundaries. A running task may finish with the
    /// previous narrower set and may migrate within that set. Membership is cumulative:
    /// processors are never removed, and there is no cross-region work stealing.
    ///
    /// Using the same stable, quota-appropriate processor set for eager selection and submissions
    /// keeps workers within that set. Using new processors grows the accumulated set.
    /// This does not guarantee avoidance of throttling from fractional quotas or other work.
    ///
    /// # Example
    ///
    /// ```
    /// # use std::error::Error;
    /// use new_zealand::nz;
    /// use vicinal::{Pool, WorkerPolicy};
    /// # use futures::executor::block_on;
    ///
    /// # block_on(async {
    /// // Illustrative concurrency; choose the count for your blocking workload.
    /// let pool = Pool::builder()
    ///     .worker_policy(WorkerPolicy::per_memory_region(nz!(2_u32)))
    ///     .build();
    /// assert_eq!(pool.scheduler().spawn(|| 42).await?, 42);
    /// # Ok::<(), Box<dyn Error>>(())
    /// # }).unwrap();
    /// ```
    #[must_use]
    pub fn per_memory_region(count: NonZero<u32>) -> Self {
        Self {
            kind: PolicyKind::MemoryRegion,
            count,
        }
    }

    /// Uses a fixed worker count.
    ///
    /// Workers may run only on processors selected by
    /// [`PoolBuilder::eager`][crate::PoolBuilder::eager] or that have submitted accepted work,
    /// regardless of memory region. Without eager selection, the first submission starts the
    /// workers on its processor; new submitting processors expand affinity without adding workers.
    /// Separate pools have independent worker counts and permitted processor sets.
    ///
    /// Workers refresh affinity at task boundaries. A running task may finish with the
    /// previous narrower set and may migrate within that set, including across memory regions.
    /// Membership is cumulative: processors are never removed.
    ///
    /// Using the same stable, quota-appropriate processor set for eager selection and submissions
    /// keeps workers within that set. Using new processors grows the accumulated set.
    /// This does not guarantee avoidance of throttling from fractional quotas or other work.
    ///
    /// # Example
    ///
    /// ```
    /// # use std::error::Error;
    /// use new_zealand::nz;
    /// use vicinal::{Pool, WorkerPolicy};
    /// # use futures::executor::block_on;
    ///
    /// # block_on(async {
    /// // Illustrative concurrency; choose the count for your blocking workload.
    /// let pool = Pool::builder()
    ///     .worker_policy(WorkerPolicy::per_system(nz!(2_u32)))
    ///     .build();
    /// assert_eq!(pool.scheduler().spawn(|| 42).await?, 42);
    /// # Ok::<(), Box<dyn Error>>(())
    /// # }).unwrap();
    /// ```
    #[must_use]
    pub fn per_system(count: NonZero<u32>) -> Self {
        Self {
            kind: PolicyKind::System,
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
            PolicyKind::System => SubmissionTarget {
                group: GroupId::System,
                processor: hardware.current_processor_id(),
            },
        }
    }

    pub(crate) fn kind(&self) -> PolicyKind {
        self.kind
    }

    /// Shared worker groups expand affinity as new processors submit accepted work.
    #[cfg_attr(test, mutants::skip)] // Disabling refresh can strand revision-sensitive workers.
    pub(crate) fn shares_processors(&self) -> bool {
        matches!(self.kind, PolicyKind::MemoryRegion | PolicyKind::System)
    }

    pub(crate) fn count(&self) -> NonZero<u32> {
        self.count
    }
}

/// Keeps policy variants out of the public API so extensions need not change caller matches.
#[derive(Clone, Copy, Debug)]
pub(crate) enum PolicyKind {
    Processor,
    MemoryRegion,
    System,
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
