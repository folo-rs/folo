//! Public participation handles for policy-owned worker threads.

use std::any::type_name;
use std::fmt::{Debug, Formatter, Result as FmtResult};
use std::sync::Arc;

use many_cpus::ProcessorSet;

use crate::PoolRuntime;
use crate::worker_policy::WorkerGroup;

/// A single-use reservation for one policy-owned worker thread.
///
/// [`WorkerGroup::add_worker`][crate::worker_policy::WorkerGroup::add_worker] reserves capacity
/// before the policy creates or selects a thread. Moving this value into the thread and calling
/// [`run`][Self::run] transfers that reservation into Vicinal's participation and shutdown
/// barriers. The policy retains ownership of thread creation and joining outside this API.
///
/// A worker reserved while [`WorkerPolicy::bind`][crate::worker_policy::WorkerPolicy::bind] runs
/// must be run or dropped before binding completes so construction can establish whether every
/// selected worker initialized. Dropping an unrun reservation cancels that binding-time target.
#[must_use = "reserved workers must be run or explicitly dropped"]
pub struct Worker {
    group: WorkerGroup,
    reservation: BindingReservation,
}

impl Worker {
    pub(crate) fn new(group: WorkerGroup, runtime: Arc<PoolRuntime>, active: bool) -> Self {
        Self {
            group,
            reservation: BindingReservation::new(runtime, active),
        }
    }

    /// Enters the pool's worker lifecycle on the current thread.
    ///
    /// The method blocks while this worker participates in task execution and returns when the
    /// worker leaves during shutdown or after an infrastructure failure. The policy must apply
    /// its intended initial affinity before calling this method and must not call it more than once.
    #[inline]
    pub fn run(self) {
        self.run_with(|| {}, ProcessorSet::pin_current_thread_to, None);
    }

    pub(crate) fn run_with(
        self,
        initial_setup: impl FnOnce(),
        pin: impl FnMut(&ProcessorSet) + Send,
        applied_revision: Option<usize>,
    ) {
        let Self { group, reservation } = self;
        group.execute_worker(reservation, initial_setup, pin, applied_revision);
    }
}

impl Debug for Worker {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct(type_name::<Self>())
            .field("group", &self.group)
            .field("binding_pending", &self.reservation.is_binding_pending())
            .finish()
    }
}

/// Owns one builder readiness target until a reserved worker initializes or is abandoned.
///
/// Pool binding can reserve workers that a custom policy later decides not to run.
/// Dropping this guard cancels that outstanding target, while initialization consumes it
/// without releasing an additional reservation.
pub(crate) struct BindingReservation {
    runtime: Option<Arc<PoolRuntime>>,
}

impl BindingReservation {
    fn new(runtime: Arc<PoolRuntime>, active: bool) -> Self {
        Self {
            runtime: if active { Some(runtime) } else { None },
        }
    }

    pub(crate) fn initialize(&mut self) {
        self.complete();
    }

    /// Consumes the same readiness target whether initialization finished or was abandoned.
    fn complete(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.complete_binding_worker();
        }
    }

    pub(crate) fn is_binding_pending(&self) -> bool {
        self.runtime.is_some()
    }
}

impl Drop for BindingReservation {
    fn drop(&mut self) {
        self.complete();
    }
}
