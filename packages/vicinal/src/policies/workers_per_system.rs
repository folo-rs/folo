use std::any::type_name;
use std::fmt::{Debug, Formatter, Result as FmtResult};
use std::num::NonZero;

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
}

impl Debug for WorkersPerSystem {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct(type_name::<Self>())
            .field("workers", &self.policy.worker_count())
            .finish()
    }
}
