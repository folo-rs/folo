use std::any::type_name;
use std::fmt::{Debug, Formatter, Result as FmtResult};
use std::num::NonZero;

use crate::BuiltInWorkerPolicy;
use crate::worker_policy::{BindContext, BindError, BoundPolicy, WorkerPolicy};

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
    fn bind(self, context: BindContext<'_>) -> Result<impl BoundPolicy, BindError> {
        self.policy.bind(context)
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
                .worker_policy(WorkersPerProcessor::new(nz!(1)))
                .build()
                .unwrap();

            assert_eq!(block_on(pool.scheduler().spawn(|| 42)).unwrap(), 42);
        });
    }

    #[test]
    fn formats_policy() {
        _ = format!("{:?}", WorkersPerProcessor::new(nz!(1)));
    }
}
