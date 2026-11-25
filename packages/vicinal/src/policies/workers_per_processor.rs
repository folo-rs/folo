use std::any::type_name;
use std::fmt::{Debug, Formatter, Result as FmtResult};
use std::num::NonZero;

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
}

impl Debug for WorkersPerProcessor {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct(type_name::<Self>())
            .field("workers", &self.policy.worker_count())
            .finish()
    }
}
