use std::any::type_name;
use std::fmt::{Debug, Formatter, Result as FmtResult};
use std::num::NonZero;

use crate::{
    BuiltInWorkerPolicy, PrepareContext, PrepareError, WorkerGroupId, WorkerGroups, WorkerPolicy,
};

/// Creates a fixed worker count for each submitting memory region.
///
/// The region's affinity starts with its eager processors or first submitter and expands
/// cumulatively when processors in that region reach policy selection.
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
    fn prepare(&mut self, context: PrepareContext<'_>) -> Result<(), PrepareError> {
        WorkerPolicy::prepare(&mut self.policy, context)
    }

    #[inline]
    fn select_group(&self, groups: &WorkerGroups) -> WorkerGroupId {
        WorkerPolicy::select_group(&self.policy, groups)
    }
}

impl Debug for WorkersPerMemoryRegion {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct(type_name::<Self>())
            .field("workers", &self.policy.worker_count())
            .finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use futures::executor::block_on;
    use many_cpus::SystemHardware;
    use many_cpus::fake::HardwareBuilder;
    use new_zealand::nz;
    use testing::with_watchdog;

    use super::*;
    use crate::Pool;

    #[test]
    fn executes_tasks() {
        with_watchdog(|| {
            let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
            let pool = Pool::builder()
                .hardware(hardware)
                .worker_policy(WorkersPerMemoryRegion::new(nz!(1)))
                .build();

            assert_eq!(block_on(pool.scheduler().spawn(|| 42)).unwrap(), 42);
        });
    }

    #[test]
    fn formats_policy() {
        _ = format!("{:?}", WorkersPerMemoryRegion::new(nz!(2)));
    }
}
