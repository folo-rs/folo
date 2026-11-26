//! Shared state and participation handles for one worker destination.

use std::any::type_name;
use std::collections::VecDeque;
use std::fmt::{Debug, Formatter, Result as FmtResult};
#[cfg(test)]
use std::io;
use std::num::NonZero;
use std::panic::{AssertUnwindSafe, catch_unwind};
#[cfg(test)]
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::ptr::NonNull;
#[cfg(test)]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::Builder;
#[cfg(test)]
use std::thread::JoinHandle;

use event_listener::Event;
use events_once::EventLake;
use many_cpus::ProcessorSet;
use plurality::MultiPool;
use tracing::debug;

use crate::pool_inner::{POOL_LOG_TARGET, worker_loop_with};
use crate::{ErasedTaskHandle, GroupId, NEVER_POISONED, PoolRuntime};

/// Identifies a worker group within one [`WorkerGroups`][crate::WorkerGroups] collection.
///
/// This value is [`Copy`], so policies can route tasks without cloning a group handle.
/// IDs from another pool are rejected when the pool resolves the selected group.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct WorkerGroupId {
    pub(crate) owner: u64,
    /// Stable address used by the owning registry for lock-free, ref-count-free resolution.
    ///
    /// An index into the registry's `Vec<WorkerGroup>` would be an alternative safe identity.
    /// Resolving it would require locking that vector and cloning its `Arc` on every submission
    /// because the vector may grow and references to its elements cannot outlive its guard.
    /// Each state is instead independently heap-allocated and retained for the registry's
    /// lifetime, so only the pointer and the registry owner check cross the routing boundary.
    pub(crate) state: NonNull<WorkerGroupState>,
}

impl WorkerGroupId {
    pub(crate) fn new(owner: u64, state: &Arc<WorkerGroupState>) -> Self {
        Self {
            owner,
            state: NonNull::from(Arc::as_ref(state)),
        }
    }
}

// SAFETY: Moving an ID copies only its owner and address; the ID does not dereference or own
// the pointee. The pointer is dereferenced only by WorkerGroups::resolve, after checking the
// registry owner. That registry retains the Arc-backed state for the returned shared reference's
// entire lifetime. Future direct dereferences must preserve the same checked-resolution boundary.
unsafe impl Send for WorkerGroupId {}
// SAFETY: Sharing an ID copies only its owner and address. No operation on the ID reads the
// pointee: WorkerGroups::resolve performs the sole dereference after owner validation and under
// the registry borrow that keeps the Arc-backed state alive. Sharing the ID itself neither prolongs
// nor assumes the pointee's lifetime.
unsafe impl Sync for WorkerGroupId {}

impl Debug for WorkerGroupId {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct(type_name::<Self>())
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

/// A cloneable participation handle for one worker group.
///
/// Cloning a handle is intended for transferring it to a newly created worker thread.
/// Task routing uses [`WorkerGroupId`] instead, so ordinary scheduling does not clone
/// this value.
#[derive(Clone)]
pub struct WorkerGroup {
    state: Arc<WorkerGroupState>,
    runtime: Arc<PoolRuntime>,
}

impl WorkerGroup {
    pub(crate) fn new(runtime: Arc<PoolRuntime>, destination: Option<GroupId>) -> Self {
        let state = Arc::new(WorkerGroupState::new(destination));
        runtime.register_group(&state);

        Self { state, runtime }
    }

    /// Returns the copyable identity of this group.
    #[must_use]
    #[inline]
    pub fn id(&self) -> WorkerGroupId {
        let owner = self.runtime.registry_owner();
        WorkerGroupId::new(owner, &self.state)
    }

    /// Returns the number of workers currently participating in this group.
    ///
    /// This includes workers still running their initializer. It excludes reserved workers
    /// that have not started participating.
    #[must_use]
    #[inline]
    pub fn worker_count(&self) -> usize {
        self.state.worker_count.load(Ordering::Acquire)
    }

    /// Reserves one worker that a policy must run.
    ///
    /// A worker reserved while [`WorkerPolicy::prepare`][crate::WorkerPolicy::prepare] runs
    /// remains pending until its [`Worker::run`] call completes initialization. The pool builder
    /// waits for every such reservation after preparation returns. Drop the worker to cancel
    /// its preparation target when the policy cannot run it. Retaining an unrun worker makes
    /// pool construction wait indefinitely.
    #[inline]
    pub fn add_worker(&self) -> Worker {
        Worker {
            group: self.clone(),
            reservation: PreparationReservation::new(
                Arc::clone(&self.runtime),
                self.runtime.reserve_preparation_worker(),
            ),
        }
    }

    /// Returns the combined number of urgent and regular tasks currently queued in this group.
    ///
    /// This is an instantaneous observation for policy decisions, not a reservation. Tasks
    /// and policy callbacks may change the queues immediately before or after this method
    /// returns. The task currently being submitted is not included because policy selection
    /// happens before its admission to a queue.
    #[must_use]
    pub fn queue_len(&self) -> usize {
        // Both queues are held to prevent workers from changing the observed total between
        // the two lengths. Existing queue operations lock at most one queue at a time.
        let urgent = self.state.urgent_queue.lock().expect(NEVER_POISONED);
        let regular = self.state.regular_queue.lock().expect(NEVER_POISONED);

        urgent
            .len()
            .checked_add(regular.len())
            .expect("two VecDeque lengths cannot exceed addressable memory")
    }

    fn execute_worker(
        &self,
        reservation: PreparationReservation,
        initial_setup: impl FnOnce(),
        pin: impl FnMut(&ProcessorSet) + Send,
    ) {
        if self.runtime.begin_worker().is_break() {
            return;
        }
        if let Err(payload) = catch_unwind(AssertUnwindSafe(initial_setup)) {
            self.runtime.fail(payload);
            self.runtime.end_worker();
            return;
        }
        // Acquire observes the mask revision that was released before this worker applied its
        // initial pin. The caller prevents route publication until this worker records the
        // revision before participation; later task-boundary refreshes advance only its local
        // cursor.
        let applied_revision = self.state.affinity_revision.load(Ordering::Acquire);
        self.run_worker(reservation, pin, applied_revision);
    }

    /// Runs one already-registered worker through initialization, work and cleanup.
    ///
    /// The runtime participation count remains held until failure containment and group
    /// accounting finish. Ref: docs/implementation.md, "Affinity and failure coordination".
    fn run_worker(
        &self,
        mut reservation: PreparationReservation,
        pin: impl FnMut(&ProcessorSet) + Send,
        applied_revision: usize,
    ) {
        #[cfg(test)]
        let mut initialized = false;
        let mut pin = pin;
        let result = catch_unwind(AssertUnwindSafe(|| {
            // This counter only assigns diagnostic labels; runtime participation publishes the
            // worker-count handshake separately.
            let worker_index = self.state.next_worker_index.fetch_add(1, Ordering::Relaxed);
            // Startup tracing can re-enter submission on this same route. Participation
            // must already satisfy the lazy startup waiter before the subscriber runs so
            // it can finish the route's OnceLock initialization.
            self.mark_worker_participating();
            #[cfg(test)]
            if let Some(before_startup_trace) = self.state.before_startup_trace.get() {
                before_startup_trace();
            }
            self.trace_worker_started(worker_index);

            if let Some(thread_init) = self.runtime.thread_init() {
                (thread_init.0)();
            }

            self.mark_worker_initialized(&mut reservation);
            #[cfg(test)]
            {
                initialized = true;
            }
            worker_loop_with(
                &self.runtime,
                &self.state,
                worker_index,
                applied_revision,
                &mut pin,
            );

            self.trace_worker_exiting(worker_index);
        }));

        if let Err(payload) = result {
            self.runtime.fail(payload);
            self.state.notify_initialization_failure();
            #[cfg(test)]
            if let Some(before_exit) = self.state.before_failure_exit.get() {
                before_exit();
            }
        }

        #[cfg(test)]
        if initialized {
            // This test-only Release publishes the removal from the initialized snapshot. It
            // occurs outside the initialization mutex because the counter observes completion,
            // not a condition-variable shutdown predicate.
            self.state
                .initialized_workers
                .fetch_sub(1, Ordering::Release);
        }
        self.mark_worker_finished();
        self.runtime.end_worker();
    }

    /// Records the start of a worker while preserving destination-specific trace fields.
    fn trace_worker_started(&self, worker_index: u32) {
        if let Some(group) = self.state.destination {
            debug!(
                target: POOL_LOG_TARGET,
                pool_name = self.runtime.pool_name(),
                pool_id = self.runtime.pool_id(),
                group = ?group,
                worker_index,
                "worker thread started"
            );
        } else {
            debug!(
                target: POOL_LOG_TARGET,
                pool_name = self.runtime.pool_name(),
                pool_id = self.runtime.pool_id(),
                worker_index,
                "worker thread started"
            );
        }
    }

    /// Records the end of a worker while preserving destination-specific trace fields.
    fn trace_worker_exiting(&self, worker_index: u32) {
        if let Some(group) = self.state.destination {
            debug!(
                target: POOL_LOG_TARGET,
                pool_name = self.runtime.pool_name(),
                pool_id = self.runtime.pool_id(),
                group = ?group,
                worker_index,
                "worker thread exiting"
            );
        } else {
            debug!(
                target: POOL_LOG_TARGET,
                pool_name = self.runtime.pool_name(),
                pool_id = self.runtime.pool_id(),
                worker_index,
                "worker thread exiting"
            );
        }
    }

    // Replacing lazy startup with an empty body can strand joined submissions forever because no
    // participation source remains for the readiness wait. Ref: docs/testing.md, "Tests must not
    // hang".
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn start_workers(
        &self,
        destination: GroupId,
        affinity: &ProcessorSet,
        count: NonZero<u32>,
    ) {
        // Ref: docs/implementation.md, "Worker initialization".
        // Lazy startup reserves participation only; initializer completion remains a separate
        // eager/custom policy target.
        let expected = self
            .state
            .worker_count
            .load(Ordering::Acquire)
            .checked_add(count.get() as usize)
            .expect("worker reservation is bounded by the topology implementation");
        self.spawn_workers(destination, affinity, count);
        self.wait_for_workers(expected);
        #[cfg(test)]
        {
            _ = self.state.startup.get_or_init(|| Ok(()));
        }
    }

    // Omitting eager startup can leave a successfully constructed policy-selected route with
    // no worker to execute admitted tasks.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn start_eager_workers(
        &self,
        destination: GroupId,
        affinity: &ProcessorSet,
        count: NonZero<u32>,
    ) {
        self.spawn_workers(destination, affinity, count);
    }

    // Removing thread creation leaves the lazy readiness wait without a producer.
    #[cfg_attr(test, mutants::skip)]
    fn spawn_workers(&self, destination: GroupId, affinity: &ProcessorSet, count: NonZero<u32>) {
        for worker_index in 0..count.get() {
            let entry = worker_entry(
                self.add_worker(),
                affinity.clone(),
                ProcessorSet::pin_current_thread_to,
            );
            let builder = Builder::new().name(format!(
                "{}-{destination}-{worker_index}",
                self.runtime.pool_name()
            ));
            let handle = builder.spawn(entry).unwrap_or_else(|error| {
                panic!("failed to start worker in {destination:?}: {error}")
            });
            _ = handle;
        }
    }

    // Omitting the participation publication can leave lazy startup waiting forever.
    #[cfg_attr(test, mutants::skip)]
    fn mark_worker_participating(&self) {
        // The readiness condition and its notification share the same mutex with the waiter,
        // closing the predicate-check/wait interval.
        let progress = self
            .state
            .initialization
            .lock()
            .expect("worker initialization state remains valid");
        self.state.worker_count.fetch_add(1, Ordering::Release);
        self.state.initialization_changed.notify_all();
        drop(progress);
    }

    // Omitting the initializer-completion publication can strand eager waiters indefinitely.
    #[cfg_attr(test, mutants::skip)]
    fn mark_worker_initialized(&self, reservation: &mut PreparationReservation) {
        #[cfg(test)]
        {
            // The test-only counter is published under the mutex and before its notification.
            // Its waiter acquires while holding the same mutex, closing the predicate/wait gap.
            let progress = self
                .state
                .initialization
                .lock()
                .expect("worker initialization state remains valid");
            self.state
                .initialized_workers
                .fetch_add(1, Ordering::Release);
            self.state.initialization_changed.notify_all();
            if reservation.is_preparation_pending() {
                _ = self.state.startup.get_or_init(|| Ok(()));
            }

            // Preserve the original test-build ordering: the initialization waiter cannot
            // observe the published count until the production reservation is complete too.
            reservation.initialize();
            drop(progress);
        }

        // The runtime reservation carries the production initialization barrier independently
        // of the test-only initialized-worker counter.
        #[cfg(not(test))]
        {
            _ = self;
            reservation.initialize();
        }
    }

    // Omitting worker-count removal can make shutdown wait forever for a nonexistent decrement.
    #[cfg_attr(test, mutants::skip)]
    fn mark_worker_finished(&self) {
        // Participation can terminate only under the readiness mutex so a waiter cannot miss
        // the terminal transition.
        let progress = self
            .state
            .initialization
            .lock()
            .expect("worker initialization state remains valid");
        self.state.worker_count.fetch_sub(1, Ordering::Release);
        self.state.initialization_changed.notify_all();
        drop(progress);
    }

    // A wrong participation predicate can wait for a worker that startup never creates.
    #[cfg_attr(test, mutants::skip)]
    fn wait_for_workers(&self, expected: usize) {
        self.wait_for_worker_count(expected, || {
            // Acquire pairs with the participating worker's mutex-protected Release.
            self.state.worker_count.load(Ordering::Acquire)
        });
    }

    #[cfg(test)]
    pub(crate) fn wait_for_initialized_workers(&self, count: NonZero<usize>) {
        self.wait_for_worker_count(count.get(), || {
            // Acquire pairs with the test-only initializer-completion Release.
            self.state.initialized_workers.load(Ordering::Acquire)
        });
    }

    // A wrong predicate or a skipped wake can strand every readiness waiter without another
    // producer. The condition-variable and mutex keep each transition observable by a waiter.
    #[cfg_attr(test, mutants::skip)]
    fn wait_for_worker_count(&self, expected: usize, ready: impl Fn() -> usize) {
        let mut progress = self
            .state
            .initialization
            .lock()
            .expect("worker initialization state remains valid");
        loop {
            if ready() >= expected {
                return;
            }
            if self.runtime.is_poisoned() {
                drop(progress);
                panic!("worker pool failed before readiness");
            }
            if self.runtime.is_shutdown() {
                return;
            }

            progress = self
                .state
                .initialization_changed
                .wait(progress)
                .expect("worker initialization state remains valid");
        }
    }

    #[cfg(test)]
    // Removing the injected startup path can leave its readiness wait without a producer.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn start_workers_with(
        &self,
        destination: GroupId,
        affinity: &ProcessorSet,
        count: NonZero<u32>,
        handles: &mut Vec<JoinHandle<()>>,
        mut spawn: impl FnMut(Builder, WorkerEntry) -> io::Result<JoinHandle<()>>,
        pin: impl Fn(&ProcessorSet) + Clone + Send + 'static,
    ) {
        let expected = self
            .state
            .worker_count
            .load(Ordering::Acquire)
            .checked_add(count.get() as usize)
            .expect("worker reservation is bounded by the topology implementation");

        for worker_index in 0..count.get() {
            let entry: WorkerEntry = Box::new(worker_entry(
                self.add_worker(),
                affinity.clone(),
                pin.clone(),
            ));
            let builder = Builder::new().name(format!(
                "{}-{destination}-{worker_index}",
                self.runtime.pool_name()
            ));
            let handle = spawn(builder, entry).unwrap_or_else(|error| {
                panic!("failed to start worker in {destination:?}: {error}")
            });
            handles.push(handle);
        }

        self.wait_for_workers(expected);
        _ = self.state.startup.get_or_init(|| Ok(()));
    }

    pub(crate) fn state(&self) -> &WorkerGroupState {
        &self.state
    }

    #[cfg(test)]
    pub(crate) fn get_tasks_spawned(state: &WorkerGroupState) -> u64 {
        state.get_tasks_spawned()
    }
}

impl Debug for WorkerGroup {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct(type_name::<Self>())
            .field("id", &self.id())
            .field("workers", &self.worker_count())
            .finish_non_exhaustive()
    }
}

/// Builds the concrete thread entry before the chosen spawn boundary.
fn worker_entry(
    worker: Worker,
    affinity: ProcessorSet,
    pin: impl Fn(&ProcessorSet) + Clone + Send + 'static,
) -> impl FnOnce() + Send + 'static {
    let initial_pin = pin.clone();

    move || worker.run_with(move || initial_pin(&affinity), pin)
}

/// Runs one reserved worker through the pool's worker lifecycle.
///
/// A policy moves this value into a thread and calls [`run`][Self::run] once.
/// Dropping it before that call cancels any worker-initialization target created during
/// policy preparation.
#[must_use = "reserved workers must be run or explicitly dropped"]
pub struct Worker {
    group: WorkerGroup,
    reservation: PreparationReservation,
}

impl Debug for Worker {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct(type_name::<Self>())
            .field("group", &self.group)
            .field(
                "preparation_pending",
                &self.reservation.is_preparation_pending(),
            )
            .finish()
    }
}

impl Worker {
    /// Runs this worker on the current thread until the pool shuts down.
    ///
    /// The policy must apply the intended initial affinity before calling this method.
    #[inline]
    pub fn run(self) {
        self.run_with(|| {}, ProcessorSet::pin_current_thread_to);
    }

    fn run_with(self, initial_setup: impl FnOnce(), pin: impl FnMut(&ProcessorSet) + Send) {
        let Self { group, reservation } = self;
        group.execute_worker(reservation, initial_setup, pin);
    }
}

/// Releases an outstanding preparation target when a reserved worker is dropped.
struct PreparationReservation {
    runtime: Option<Arc<PoolRuntime>>,
}

impl PreparationReservation {
    fn new(runtime: Arc<PoolRuntime>, active: bool) -> Self {
        Self {
            runtime: if active { Some(runtime) } else { None },
        }
    }

    fn initialize(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.initialize_preparation_worker();
        }
    }

    fn is_preparation_pending(&self) -> bool {
        self.runtime.is_some()
    }
}

impl Drop for PreparationReservation {
    fn drop(&mut self) {
        if let Some(runtime) = self.runtime.take() {
            runtime.cancel_preparation_worker();
        }
    }
}

/// Everything a group's worker threads share.
pub(crate) struct WorkerGroupState {
    pub(crate) destination: Option<GroupId>,

    pub(crate) urgent_queue: Mutex<VecDeque<ErasedTaskHandle>>,
    pub(crate) regular_queue: Mutex<VecDeque<ErasedTaskHandle>>,
    pub(crate) wake_event: Event,
    pub(crate) shutdown_flag: AtomicBool,
    pub(crate) affinity: RwLock<Option<Arc<ProcessorSet>>>,
    pub(crate) affinity_revision: AtomicUsize,

    pub(crate) task_pool: Mutex<MultiPool>,
    pub(crate) result_channel_pool: EventLake,
    pub(crate) tasks_spawned: AtomicU64,

    next_worker_index: AtomicU32,
    // Acquire observations in worker_count(), lazy-startup reservation and the mutex-held
    // waiter pair with Release updates made under initialization. That mutex is also required
    // to keep the predicate check and condition-variable wait from missing an increment or
    // terminal decrement.
    worker_count: AtomicUsize,
    #[cfg(test)]
    // Test-only initializer-completion observations. Completion publishes Release while holding
    // initialization; the startup test acquires while holding that same mutex. Exit removal is
    // outside the mutex and publishes only a current-snapshot decrement, not a readiness wakeup.
    initialized_workers: AtomicUsize,
    #[cfg(test)]
    pub(crate) startup: OnceLock<Result<(), ()>>,
    #[cfg(test)]
    pub(crate) before_startup_trace:
        OnceLock<Box<dyn Fn() + Send + Sync + UnwindSafe + RefUnwindSafe>>,
    #[cfg(test)]
    pub(crate) before_wait:
        OnceLock<Box<dyn Fn(u32, usize) + Send + Sync + UnwindSafe + RefUnwindSafe>>,
    #[cfg(test)]
    pub(crate) before_failure_exit:
        OnceLock<Box<dyn Fn() + Send + Sync + UnwindSafe + RefUnwindSafe>>,
    #[cfg(test)]
    pub(crate) before_membership_publish:
        OnceLock<Box<dyn Fn() + Send + Sync + UnwindSafe + RefUnwindSafe>>,
    #[cfg(test)]
    pub(crate) before_membership_lock:
        OnceLock<Box<dyn Fn() + Send + Sync + UnwindSafe + RefUnwindSafe>>,
    initialization: Mutex<()>,
    initialization_changed: Condvar,
}

impl WorkerGroupState {
    pub(crate) fn new(destination: Option<GroupId>) -> Self {
        Self {
            destination,
            urgent_queue: Mutex::new(VecDeque::new()),
            regular_queue: Mutex::new(VecDeque::new()),
            wake_event: Event::new(),
            shutdown_flag: AtomicBool::new(false),
            affinity: RwLock::new(None),
            affinity_revision: AtomicUsize::new(0),
            task_pool: Mutex::new(MultiPool::new()),
            result_channel_pool: EventLake::new(),
            tasks_spawned: AtomicU64::new(0),
            next_worker_index: AtomicU32::new(0),
            worker_count: AtomicUsize::new(0),
            #[cfg(test)]
            initialized_workers: AtomicUsize::new(0),
            #[cfg(test)]
            startup: OnceLock::new(),
            #[cfg(test)]
            before_startup_trace: OnceLock::new(),
            #[cfg(test)]
            before_wait: OnceLock::new(),
            #[cfg(test)]
            before_failure_exit: OnceLock::new(),
            #[cfg(test)]
            before_membership_publish: OnceLock::new(),
            #[cfg(test)]
            before_membership_lock: OnceLock::new(),
            initialization: Mutex::new(()),
            initialization_changed: Condvar::new(),
        }
    }

    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn signal_shutdown(&self) {
        // This group-local terminal bit is distinct from the runtime shutdown and poison flags.
        // The worker's post-listener Acquire observes this Release and the caller's preceding
        // terminal-state writes, then the event notification closes the register/recheck/wait
        // gap. The atomic does not replace queue locks or publish all queue access.
        self.shutdown_flag.store(true, Ordering::Release);
        self.wake_event.notify(usize::MAX);
    }

    pub(crate) fn record_task_spawned(&self) {
        // This is a value-only monotonic observation counter; no other state is published
        // through it, so Relaxed is sufficient.
        self.tasks_spawned.fetch_add(1, Ordering::Relaxed);
    }

    pub(crate) fn notify_initialization_failure(&self) {
        let progress = self
            .initialization
            .lock()
            .expect("worker initialization state remains valid");
        self.initialization_changed.notify_all();
        drop(progress);
    }

    #[cfg(test)]
    pub(crate) fn get_tasks_spawned(&self) -> u64 {
        // Pair with record_task_spawned(): tests inspect only the counter value.
        self.tasks_spawned.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn mark_startup_failed_if_started(&self, poisoned: bool) {
        if poisoned {
            _ = self.startup.set(Err(()));
        }
    }
}

/// Public group handles expose only participation and identity; queue internals stay borrowed.
pub(crate) type GroupState = WorkerGroupState;

/// Erases the shared worker-spawn entry only at injected test spawn boundaries.
#[cfg(test)]
pub(crate) type WorkerEntry = Box<dyn FnOnce() + Send>;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::sync::OnceLock;

    use many_cpus::SystemHardware;
    use many_cpus::fake::HardwareBuilder;
    use new_zealand::nz;
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use crate::{
        Pool, PrepareContext, PrepareError, Worker, WorkerGroupId, WorkerGroups, WorkerPolicy,
    };

    assert_impl_all!(Worker: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(Worker: Copy, Clone);

    /// Supplies one routable group without workers so admitted tasks remain observable.
    #[derive(Default)]
    struct NoWorkerPolicy {
        group: OnceLock<WorkerGroupId>,
    }

    impl WorkerPolicy for NoWorkerPolicy {
        fn prepare(&mut self, context: PrepareContext<'_>) -> Result<(), PrepareError> {
            self.group
                .set(context.groups().add().id())
                .expect("the no-worker policy prepares once");
            Ok(())
        }

        fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
            *self
                .group
                .get()
                .expect("the builder prepares the no-worker policy")
        }
    }

    #[test]
    fn queue_len_combines_both_priorities() {
        let pool = Pool::builder()
            .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
                nz!(1),
                nz!(1),
            )))
            .worker_policy(NoWorkerPolicy::default())
            .build();
        let group = pool
            .inner
            .groups
            .iter()
            .next()
            .expect("the policy creates one group");
        let scheduler = pool.scheduler();

        let _regular = scheduler.spawn(|| ());
        let _urgent = scheduler.spawn_urgent(|| ());

        assert_eq!(group.queue_len(), 2);
    }
}
