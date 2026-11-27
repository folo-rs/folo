//! Shared state and participation handles for one worker destination.

use std::any::type_name;
use std::collections::VecDeque;
use std::fmt::{Debug, Formatter, Result as FmtResult};
use std::io;
use std::num::NonZero;
use std::panic::{AssertUnwindSafe, catch_unwind};
#[cfg(test)]
use std::panic::{RefUnwindSafe, UnwindSafe};
#[cfg(test)]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
#[cfg(test)]
use std::sync::mpsc::Sender;
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
use crate::worker_groups::GroupRegistration;
use crate::worker_policy::{Worker, WorkerGroupId};
use crate::{
    BindingReservation, ErasedTaskHandle, GroupId, NEVER_POISONED, PanicFailures, PoolRuntime,
};

/// A cloneable participation handle for one worker group.
///
/// Cloning a handle is intended for transferring it to a newly created worker thread.
/// Task routing uses [`WorkerGroupId`] instead, so ordinary scheduling does not clone
/// this value.
#[derive(Clone)]
pub struct WorkerGroup {
    id: WorkerGroupId,
    state: Arc<WorkerGroupState>,
    runtime: Arc<PoolRuntime>,
}

impl WorkerGroup {
    pub(crate) fn new(runtime: Arc<PoolRuntime>, registration: GroupRegistration) -> Self {
        let (id, state) = registration.into_parts();
        runtime.register_group(&state);

        Self { id, state, runtime }
    }

    /// Returns the copyable identity of this group.
    #[must_use]
    #[inline]
    pub fn id(&self) -> WorkerGroupId {
        self.id
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
    /// A worker reserved while
    /// [`WorkerPolicy::bind`][crate::worker_policy::WorkerPolicy::bind] runs
    /// remains pending until its [`Worker::run`] call completes initialization. The pool builder
    /// waits for every such reservation after binding returns. Drop the worker to cancel
    /// its binding target when the policy cannot run it. Retaining an unrun worker makes
    /// pool construction wait indefinitely.
    #[inline]
    pub fn add_worker(&self) -> Worker {
        Worker::new(
            self.clone(),
            Arc::clone(&self.runtime),
            self.runtime.reserve_binding_worker(),
        )
    }

    /// Lets the cancellation fixture wait for a live builder readiness wait.
    #[cfg(test)]
    pub(crate) fn observe_binding_wait(&self, waiting: Sender<()>) {
        self.runtime.before_binding_wait.set(waiting).unwrap();
    }

    /// Lets a parked fixture worker resume once failed construction waits for its departure.
    #[cfg(test)]
    pub(crate) fn observe_worker_wait(&self, waiting: Sender<()>) {
        assert!(
            self.runtime
                .before_worker_wait
                .set(Box::new(move || {
                    _ = waiting.send(());
                }))
                .is_ok()
        );
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
            .expect("both queue allocations coexist in one addressable process space")
    }

    pub(crate) fn execute_worker(
        &self,
        reservation: BindingReservation,
        initial_setup: impl FnOnce(),
        pin: impl FnMut(&ProcessorSet) + Send,
        applied_revision: Option<usize>,
    ) {
        if self.runtime.begin_worker().is_break() {
            return;
        }
        let exit = WorkerExitGuard {
            group: self,
            participating: false,
            #[cfg(test)]
            initialized: false,
        };
        if let Err(payload) = catch_unwind(AssertUnwindSafe(initial_setup)) {
            self.runtime.fail(PanicFailures::new(payload));
            return;
        }
        // Built-in startup carries the revision paired with its captured initial mask, so an
        // expansion between pinning and this load cannot hide a refresh. Public Worker::run
        // has no captured mask to pair and observes the current revision after its user setup.
        let applied_revision = applied_revision
            .unwrap_or_else(|| self.state.affinity_revision.load(Ordering::Acquire));
        self.run_worker(reservation, pin, applied_revision, exit);
    }

    /// Runs one already-registered worker through initialization, work and cleanup.
    ///
    /// The runtime participation count remains held until failure containment and group
    /// accounting finish. Ref: docs/implementation.md, "Affinity and failure coordination".
    fn run_worker(
        &self,
        mut reservation: BindingReservation,
        pin: impl FnMut(&ProcessorSet) + Send,
        applied_revision: usize,
        mut exit: WorkerExitGuard<'_>,
    ) {
        let mut pin = pin;
        let result = catch_unwind(AssertUnwindSafe(|| {
            // This counter only assigns diagnostic labels; runtime participation publishes the
            // worker-count handshake separately.
            let worker_index = self.state.next_worker_index.fetch_add(1, Ordering::Relaxed);
            // Startup tracing can re-enter submission on this same route. Participation
            // must already satisfy the lazy startup waiter before the subscriber runs so
            // it can finish the route's OnceLock initialization.
            self.mark_worker_participating();
            exit.participating = true;
            #[cfg(test)]
            if let Some(before_startup_trace) = self.state.before_startup_trace.get() {
                before_startup_trace();
            }
            self.trace_worker_started(worker_index);

            if let Some(thread_init) = self.runtime.thread_init() {
                (thread_init.0)();
            }

            #[cfg(test)]
            let binding_pending = reservation.is_binding_pending();
            reservation.initialize();
            #[cfg(test)]
            {
                self.observe_worker_initialized(binding_pending);
                exit.initialized = true;
            }
            worker_loop_with(
                &self.runtime,
                &self.state,
                worker_index,
                applied_revision,
                &mut pin,
            )?;

            self.trace_worker_exiting(worker_index);
            Ok(())
        }));

        if let Err(failures) = result.map_err(PanicFailures::new).flatten() {
            self.runtime.fail(failures);
            self.state.notify_initialization_failure();
            #[cfg(test)]
            if let Some(before_exit) = self.state.before_failure_exit.get() {
                before_exit();
            }
        }
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
        affinity: &Arc<ProcessorSet>,
        count: NonZero<u32>,
    ) {
        let expected = self.participation_target(count);
        self.spawn_workers(destination, affinity, count)
            .unwrap_or_else(|error| panic!("failed to start worker in {destination:?}: {error}"));
        self.wait_for_workers(expected);
        #[cfg(test)]
        {
            _ = self.state.startup.get_or_init(|| Ok(()));
        }
    }

    /// Counts all participants the lazy readiness wait must account for after spawning.
    fn participation_target(&self, count: NonZero<u32>) -> usize {
        // Ref: docs/implementation.md, "Worker initialization".
        // Lazy startup reserves participation only; initializer completion remains a separate
        // eager/custom policy target.
        self.state
            .worker_count
            .load(Ordering::Acquire)
            .checked_add(count.get() as usize)
            .expect(
                "the topology count and existing workers both fit in the process's Vec capacity",
            )
    }

    /// Starts detached workers, leaving readiness and refusal handling to the caller.
    // Removing thread creation leaves the lazy readiness wait without a producer.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn spawn_workers(
        &self,
        destination: GroupId,
        affinity: &Arc<ProcessorSet>,
        count: NonZero<u32>,
    ) -> io::Result<()> {
        for (builder, entry) in self.prepare_workers(
            destination,
            affinity,
            count,
            ProcessorSet::pin_current_thread_to,
        ) {
            _ = builder.spawn(entry)?;
        }
        Ok(())
    }

    /// Prepares concrete entries that share one startup mask and its captured revision.
    fn prepare_workers(
        &self,
        destination: GroupId,
        affinity: &Arc<ProcessorSet>,
        count: NonZero<u32>,
        pin: impl Fn(&ProcessorSet) + Clone + Send + 'static,
    ) -> impl Iterator<Item = (Builder, impl FnOnce() + Send + 'static)> {
        // Pair the revision with the shared startup snapshot before any worker is spawned.
        // Ref: docs/implementation.md, "Affinity and failure coordination".
        let applied_revision = self.state.affinity_revision.load(Ordering::Acquire);
        (0..count.get()).map(move |worker_index| {
            let entry = worker_entry(
                self.add_worker(),
                Arc::clone(affinity),
                applied_revision,
                pin.clone(),
            );
            let builder = Builder::new().name(format!(
                "{}-{destination}-{worker_index}",
                self.runtime.pool_name()
            ));
            (builder, entry)
        })
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

    /// Publishes test readiness only after the production reservation has completed.
    #[cfg(test)]
    // Omitting the initializer observation can strand fixture waiters indefinitely.
    #[cfg_attr(test, mutants::skip)]
    fn observe_worker_initialized(&self, binding_pending: bool) {
        // The group mutex closes the predicate/wait gap only for the test observer.
        let progress = self
            .state
            .initialization
            .lock()
            .expect("worker initialization state remains valid");
        if binding_pending {
            _ = self.state.startup.get_or_init(|| Ok(()));
        }
        self.state
            .initialized_workers
            .fetch_add(1, Ordering::Release);
        self.state.initialization_changed.notify_all();
        drop(progress);
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
        affinity: &Arc<ProcessorSet>,
        count: NonZero<u32>,
        handles: &mut Vec<JoinHandle<()>>,
        mut spawn: impl FnMut(Builder, WorkerEntry) -> io::Result<JoinHandle<()>>,
        pin: impl Fn(&ProcessorSet) + Clone + Send + 'static,
    ) {
        let expected = self.participation_target(count);
        self.spawn_workers_with(destination, affinity, count, pin, |builder, entry| {
            handles.push(spawn(builder, entry)?);
            Ok(())
        })
        .unwrap_or_else(|error| panic!("failed to start worker in {destination:?}: {error}"));
        self.wait_for_workers(expected);
        _ = self.state.startup.get_or_init(|| Ok(()));
    }

    /// Erases worker entries only at the test boundary, where spawning can be intercepted.
    #[cfg(test)]
    pub(crate) fn spawn_workers_with(
        &self,
        destination: GroupId,
        affinity: &Arc<ProcessorSet>,
        count: NonZero<u32>,
        pin: impl Fn(&ProcessorSet) + Clone + Send + 'static,
        mut spawn: impl FnMut(Builder, WorkerEntry) -> io::Result<()>,
    ) -> io::Result<()> {
        for (builder, entry) in self.prepare_workers(destination, affinity, count, pin) {
            spawn(builder, Box::new(entry))?;
        }
        Ok(())
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

/// Releases worker lifecycle accounting if failure handling starts unwinding.
struct WorkerExitGuard<'a> {
    group: &'a WorkerGroup,
    participating: bool,
    #[cfg(test)]
    initialized: bool,
}

impl Drop for WorkerExitGuard<'_> {
    fn drop(&mut self) {
        #[cfg(test)]
        if self.initialized {
            self.group
                .state
                .initialized_workers
                .fetch_sub(1, Ordering::Release);
        }

        if self.participating {
            self.group.mark_worker_finished();
        }
        self.group.runtime.end_worker();
    }
}

/// Builds the concrete thread entry before the chosen spawn boundary.
fn worker_entry(
    worker: Worker,
    affinity: Arc<ProcessorSet>,
    applied_revision: usize,
    pin: impl Fn(&ProcessorSet) + Clone + Send + 'static,
) -> impl FnOnce() + Send + 'static {
    let initial_pin = pin.clone();

    move || worker.run_with(move || initial_pin(&affinity), pin, Some(applied_revision))
}

/// Owns all synchronized state shared by the workers in one destination group.
///
/// `WorkerGroup` handles retain this allocation, while worker threads and schedulers access
/// its queues, affinity, readiness and participation state through narrow synchronization
/// boundaries. Keeping this state separate makes group handles cloneable without copying the
/// destination's queues or worker-lifecycle accounting.
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
    #[cfg(test)]
    pub(crate) before_membership_selection:
        OnceLock<Arc<dyn Fn() + Send + Sync + UnwindSafe + RefUnwindSafe>>,
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
            #[cfg(test)]
            before_membership_selection: OnceLock::new(),
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

    use crate::Pool;
    use crate::worker_policy::{
        BindContext, BindError, BoundPolicy, Worker, WorkerGroupId, WorkerGroups, WorkerPolicy,
    };

    assert_impl_all!(Worker: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(Worker: Copy, Clone);

    /// Supplies one routable group without workers so admitted tasks remain observable.
    #[derive(Debug, Default)]
    struct NoWorkerPolicy {
        group: OnceLock<WorkerGroupId>,
    }

    impl WorkerPolicy for NoWorkerPolicy {
        fn bind(self, context: BindContext<'_>) -> Result<impl BoundPolicy, BindError> {
            self.group
                .set(context.groups().add().id())
                .expect("the no-worker policy binds once");
            Ok(self)
        }
    }

    impl BoundPolicy for NoWorkerPolicy {
        fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
            *self
                .group
                .get()
                .expect("the builder binds the no-worker policy")
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
            .build()
            .unwrap();
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
