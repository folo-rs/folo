//! Public participation handles for policy-owned worker threads.

use std::any::type_name;
use std::fmt::{Debug, Formatter, Result as FmtResult};
use std::sync::Arc;

use many_cpus::ProcessorSet;

use crate::{PoolRuntime, WorkerGroup};

/// Runs one reserved worker through the pool's worker lifecycle.
///
/// A policy moves this value into a thread and calls [`run`][Self::run] once.
/// Dropping it before that call cancels any worker-initialization target created during
/// policy binding.
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

    /// Runs this worker on the current thread until the pool shuts down.
    ///
    /// The policy must apply the intended initial affinity before calling this method.
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
    #[cfg_attr(coverage_nightly, coverage(off))]
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
        if let Some(runtime) = self.runtime.take() {
            runtime.initialize_binding_worker();
        }
    }

    pub(crate) fn is_binding_pending(&self) -> bool {
        self.runtime.is_some()
    }
}

impl Drop for BindingReservation {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.cancel_binding_worker();
        }
    }
}
