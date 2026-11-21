//! Shared state and participation handles for one worker destination.

use std::any::type_name;
use std::collections::VecDeque;
use std::num::NonZero;
use std::panic::{self, AssertUnwindSafe};
use std::ptr::NonNull;
#[cfg(test)]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::{fmt, io, thread};

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

// SAFETY: IDs contain an address into registry-owned synchronized state. The fields remain
// private, and IDs can be constructed only for a group owned by that exact registry.
unsafe impl Send for WorkerGroupId {}
// SAFETY: The target state contains synchronization for every mutable field and IDs expose
// only the immutable registry address.
unsafe impl Sync for WorkerGroupId {}

impl fmt::Debug for WorkerGroupId {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
    #[must_use]
    #[inline]
    pub fn len(&self) -> usize {
        self.state.worker_count.load(Ordering::Acquire)
    }

    /// Returns whether this group currently has no participating workers.
    ///
    /// Participating workers may still be running their initialization callback.
    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
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

    /// Runs the worker loop on the current thread until the pool shuts down.
    ///
    /// Policies normally clone a group into a thread before entering this method.
    /// Worker initialization, task execution, affinity refresh and shutdown are handled
    /// by the pool. The policy remains responsible for thread creation and for applying
    /// the intended affinity before calling this method.
    pub fn execute_worker(&self) {
        self.execute_worker_with(
            ProcessorSet::pin_current_thread_to,
            self.state.affinity_revision.load(Ordering::Acquire),
        );
    }

    fn execute_worker_with(&self, pin: impl FnMut(&ProcessorSet) + Send, applied_revision: usize) {
        if !self.runtime.begin_worker() {
            return;
        }

        let mut initialized = false;
        let mut pin = pin;
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            let worker_index = self.state.next_worker_index.fetch_add(1, Ordering::Relaxed);
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

            self.mark_worker_participating();
            if let Some(thread_init) = self.runtime.thread_init() {
                (thread_init.0)();
            }

            self.mark_worker_initialized();
            initialized = true;
            worker_loop_with(
                &self.runtime,
                &self.state,
                worker_index,
                applied_revision,
                &mut pin,
            );

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
        }));

        if let Err(payload) = result {
            self.runtime.fail(payload);
            self.state.notify_initialization_failure();
            #[cfg(test)]
            if let Some(before_exit) = self.state.before_failure_exit.get() {
                before_exit();
            }
        }

        if initialized {
            self.state
                .initialized_workers
                .fetch_sub(1, Ordering::Release);
        }
        self.mark_worker_finished();
        self.runtime.end_worker();
    }

    pub(crate) fn start_workers(
        &self,
        destination: GroupId,
        affinity: &ProcessorSet,
        count: NonZero<u32>,
    ) {
        let expected = self
            .state
            .worker_count
            .load(Ordering::Acquire)
            .checked_add(count.get() as usize)
            .expect("worker reservation is bounded by the topology implementation");
        self.spawn_workers(destination, affinity, count);
        self.wait_for_workers(expected, false);
        #[cfg(test)]
        self.state
            .startup
            .set(Ok(()))
            .expect("a worker group starts once");
    }

    pub(crate) fn start_eager_workers(
        &self,
        destination: GroupId,
        affinity: &ProcessorSet,
        count: NonZero<u32>,
    ) -> usize {
        let expected = self
            .state
            .initialized_workers
            .load(Ordering::Acquire)
            .checked_add(count.get() as usize)
            .expect("worker reservation is bounded by the topology implementation");
        self.spawn_workers(destination, affinity, count);
        expected
    }

    fn spawn_workers(&self, destination: GroupId, affinity: &ProcessorSet, count: NonZero<u32>) {
        let mut handles = Vec::new();
        self.spawn_workers_with(
            destination,
            affinity,
            count,
            &mut handles,
            thread::Builder::spawn,
            ProcessorSet::pin_current_thread_to,
        );
    }

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

    fn mark_worker_initialized(&self) {
        // Release publishes completion of thread_init; the waiter acquires while holding the
        // same condition-variable mutex.
        let progress = self
            .state
            .initialization
            .lock()
            .expect("worker initialization state remains valid");
        self.state
            .initialized_workers
            .fetch_add(1, Ordering::Release);
        self.state.initialization_changed.notify_all();
        drop(progress);
    }

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

    // A wrong readiness comparison can require a worker that a fixed-count policy never creates.
    #[cfg_attr(test, mutants::skip)]
    fn wait_for_workers(&self, expected: usize, initialized: bool) {
        let mut progress = self
            .state
            .initialization
            .lock()
            .expect("worker initialization state remains valid");
        loop {
            let ready = if initialized {
                self.state.initialized_workers.load(Ordering::Acquire)
            } else {
                self.state.worker_count.load(Ordering::Acquire)
            };
            if ready >= expected {
                return;
            }
            if self.runtime.is_poisoned() {
                drop(progress);
                self.runtime.resume_failure();
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

    pub(crate) fn wait_for_eager_workers(&self, expected: usize) {
        self.wait_for_workers(expected, true);
        #[cfg(test)]
        self.state
            .startup
            .set(Ok(()))
            .expect("a worker group starts once");
    }

    #[cfg(test)]
    pub(crate) fn start_workers_with(
        &self,
        destination: GroupId,
        affinity: &ProcessorSet,
        count: NonZero<u32>,
        handles: &mut Vec<thread::JoinHandle<()>>,
        spawn: impl FnMut(thread::Builder, WorkerEntry) -> io::Result<thread::JoinHandle<()>>,
        pin: impl Fn(&ProcessorSet) + Clone + Send + 'static,
    ) {
        let expected = self
            .state
            .worker_count
            .load(Ordering::Acquire)
            .checked_add(count.get() as usize)
            .expect("worker reservation is bounded by the topology implementation");

        self.spawn_workers_with(destination, affinity, count, handles, spawn, pin);

        self.wait_for_workers(expected, false);
        self.state
            .startup
            .set(Ok(()))
            .expect("a worker group starts once");
    }

    fn spawn_workers_with(
        &self,
        destination: GroupId,
        affinity: &ProcessorSet,
        count: NonZero<u32>,
        handles: &mut Vec<thread::JoinHandle<()>>,
        mut spawn: impl FnMut(thread::Builder, WorkerEntry) -> io::Result<thread::JoinHandle<()>>,
        pin: impl Fn(&ProcessorSet) + Clone + Send + 'static,
    ) {
        let applied_revision = self.state.affinity_revision.load(Ordering::Acquire);
        for worker_index in 0..count.get() {
            let entry: WorkerEntry = Box::new({
                let group = self.clone();
                let affinity = affinity.clone();
                let pin = pin.clone();
                move || {
                    let result = panic::catch_unwind(AssertUnwindSafe(|| pin(&affinity)));
                    if let Err(payload) = result {
                        group.runtime.fail(payload);
                        return;
                    }
                    group.execute_worker_with(pin, applied_revision);
                }
            });
            let builder = thread::Builder::new().name(format!(
                "{}-{destination}-{worker_index}",
                self.runtime.pool_name()
            ));
            let handle = spawn(builder, entry).unwrap_or_else(|error| {
                panic!("failed to start worker in {destination:?}: {error}")
            });
            handles.push(handle);
        }
    }

    pub(crate) fn state(&self) -> &WorkerGroupState {
        &self.state
    }

    #[cfg(test)]
    pub(crate) fn get_tasks_spawned(state: &WorkerGroupState) -> u64 {
        state.get_tasks_spawned()
    }
}

impl fmt::Debug for WorkerGroup {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("id", &self.id())
            .field("workers", &self.len())
            .finish_non_exhaustive()
    }
}

/// Everything a group's worker threads share.
pub(crate) struct WorkerGroupState {
    pub(crate) destination: Option<GroupId>,

    pub(crate) urgent_queue: Mutex<VecDeque<ErasedTaskHandle>>,
    pub(crate) regular_queue: Mutex<VecDeque<ErasedTaskHandle>>,
    pub(crate) wake_event: Event,
    pub(crate) shutdown_flag: AtomicBool,
    pub(crate) affinity: RwLock<Option<ProcessorSet>>,
    pub(crate) affinity_revision: AtomicUsize,

    pub(crate) task_pool: Mutex<MultiPool>,
    pub(crate) result_channel_pool: EventLake,
    pub(crate) tasks_spawned: AtomicU64,

    next_worker_index: AtomicU32,
    worker_count: AtomicUsize,
    initialized_workers: AtomicUsize,
    #[cfg(test)]
    pub(crate) startup: OnceLock<Result<(), ()>>,
    #[cfg(test)]
    pub(crate) before_wait:
        OnceLock<Box<dyn Fn(u32, usize) + Send + Sync + panic::UnwindSafe + panic::RefUnwindSafe>>,
    #[cfg(test)]
    pub(crate) before_failure_exit:
        OnceLock<Box<dyn Fn() + Send + Sync + panic::UnwindSafe + panic::RefUnwindSafe>>,
    #[cfg(test)]
    pub(crate) before_membership_publish:
        OnceLock<Box<dyn Fn() + Send + Sync + panic::UnwindSafe + panic::RefUnwindSafe>>,
    #[cfg(test)]
    pub(crate) before_membership_lock:
        OnceLock<Box<dyn Fn() + Send + Sync + panic::UnwindSafe + panic::RefUnwindSafe>>,
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
            initialized_workers: AtomicUsize::new(0),
            #[cfg(test)]
            startup: OnceLock::new(),
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

pub(crate) type WorkerEntry = Box<dyn FnOnce() + Send>;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::pin::Pin;

    use crate::{Pool, VicinalTask, init_task};

    struct NoopTask;

    impl VicinalTask for NoopTask {
        fn call(self: Pin<&mut Self>) {}
    }

    #[test]
    fn queue_len_combines_both_priorities() {
        let pool = Pool::new();
        let group = pool.inner.groups.add();
        let urgent = group.state().task_pool.lock().unwrap().alloc_uninit_box();
        let regular = group.state().task_pool.lock().unwrap().alloc_uninit_box();

        group
            .state()
            .urgent_queue
            .lock()
            .unwrap()
            .push_back(init_task(urgent, NoopTask));
        group
            .state()
            .regular_queue
            .lock()
            .unwrap()
            .push_back(init_task(regular, NoopTask));

        assert_eq!(group.queue_len(), 2);
    }
}
