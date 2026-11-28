use std::any::type_name;
use std::fmt::{Debug, Formatter, Result as FmtResult};
use std::num::NonZero;

use crate::BuiltInWorkerPolicy;
use crate::worker_policy::{BindContext, BindError, BoundPolicy, WorkerPolicy};

/// Routes submissions from one memory region to a shared fixed-size worker set.
///
/// The first submission or eager selection creates one group for a region with the configured
/// worker count. Later submissions from another processor in that region expand where those same
/// workers may run; they do not create more workers.
#[derive(Clone)]
pub struct WorkersPerMemoryRegion {
    policy: BuiltInWorkerPolicy,
}

impl WorkersPerMemoryRegion {
    /// Sets the number of workers shared by each memory region that submits work.
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
    fn bind(self, context: BindContext<'_>) -> Result<impl BoundPolicy, BindError> {
        self.policy.bind(context)
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
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use futures::executor::block_on;
    use many_cpus::SystemHardware;
    use many_cpus::fake::HardwareBuilder;
    use new_zealand::nz;
    use static_assertions::assert_impl_all;
    use testing::with_watchdog;

    use super::*;
    use crate::Pool;

    assert_impl_all!(WorkersPerMemoryRegion: Send, Sync, UnwindSafe, RefUnwindSafe);

    #[test]
    fn executes_tasks() {
        with_watchdog(|| {
            let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
            let pool = Pool::builder()
                .hardware(hardware)
                .worker_policy(WorkersPerMemoryRegion::new(nz!(1)))
                .build()
                .unwrap();

            assert_eq!(block_on(pool.scheduler().spawn(|| 42)).unwrap(), 42);
        });
    }

    #[test]
    fn formats_policy() {
        _ = format!("{:?}", WorkersPerMemoryRegion::new(nz!(2)));
    }
}
