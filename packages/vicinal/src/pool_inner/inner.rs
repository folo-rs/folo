//! Cold worker startup, shared admission and shutdown coordination.

use std::any::{Any, type_name};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle as ThreadJoinHandle, Result as ThreadResult};
use std::{fmt, io, iter, mem, panic};

use event_listener::{IntoNotification, Listener, listener};
use many_cpus::{ProcessorId, ProcessorSet, SystemHardware};
use tracing::{debug, error, trace};

use crate::{
    ErasedTaskHandle, GroupId, GroupRegistry, IterationResult, NEVER_POISONED, SubmissionTarget,
    WorkerCore, WorkerGroup, WorkerPolicy, extract_panic_message,
};

/// Keeps schedulers' shared storage alive while the owning pool controls worker lifetime.
pub(crate) struct PoolInner {
    pub(crate) pool_id: u64,
    pub(crate) pool_name: String,
    pub(crate) hardware: SystemHardware,
    pub(crate) registry: GroupRegistry,
    pub(crate) policy: WorkerPolicy,
    /// Serializes cold startup and handle registration with the shutdown transition.
    pub(crate) worker_handles: Mutex<Vec<ThreadJoinHandle<()>>>,
    pub(crate) shutdown: AtomicBool,
    /// Infrastructure failure is permanent and distinct from clean shutdown abandonment.
    pub(crate) poisoned: AtomicBool,
    /// CPUs belong to one fixed region, so one pool-wide index records regional membership.
    submitted_processors: Box<[AtomicBool]>,
}

impl PoolInner {
    pub(crate) fn new(
        pool_id: u64,
        pool_name: String,
        hardware: SystemHardware,
        policy: WorkerPolicy,
    ) -> Self {
        let submitted_processors = iter::repeat_with(|| AtomicBool::new(false))
            .take(if policy.is_regional() {
                hardware.max_processor_count()
            } else {
                0
            })
            .collect();
        Self {
            pool_id,
            pool_name,
            registry: GroupRegistry::new(&hardware, &policy),
            hardware,
            policy,
            worker_handles: Mutex::new(Vec::new()),
            shutdown: AtomicBool::new(false),
            poisoned: AtomicBool::new(false),
            submitted_processors,
        }
    }

    /// Covers topology, task storage and admission failures without catching task execution.
    #[cfg_attr(test, mutants::skip)] // Missing failure cleanup can strand accepted work.
    pub(crate) fn submit<R>(&self, action: impl FnOnce() -> R) -> R {
        assert!(!self.poisoned.load(Ordering::Acquire));
        match panic::catch_unwind(panic::AssertUnwindSafe(action)) {
            Ok(result) => result,
            Err(payload) => {
                // Startup may already have stopped the pool after releasing its registration lock.
                if self.shutdown.load(Ordering::Acquire) && self.poisoned.load(Ordering::Acquire) {
                    panic::resume_unwind(payload);
                }
                self.poison(payload);
            }
        }
    }

    /// Starts and admits work as one cold transaction; established submitters use only a queue.
    pub(crate) fn enqueue(
        self: &Arc<Self>,
        target: SubmissionTarget,
        state: &WorkerGroup,
        task: ErasedTaskHandle,
        urgent: bool,
    ) {
        self.enqueue_with(target, state, task, urgent, |handles| {
            self.start_workers(
                target,
                handles,
                thread::Builder::spawn,
                ProcessorSet::pin_current_thread_to,
            );
        });
    }

    /// Separates the startup adapter for deterministic failure and registration-race tests.
    #[cfg_attr(test, mutants::skip)] // Broken startup/registration can strand workers or submissions.
    pub(crate) fn enqueue_with(
        self: &Arc<Self>,
        target: SubmissionTarget,
        state: &WorkerGroup,
        task: ErasedTaskHandle,
        urgent: bool,
        start: impl FnOnce(&mut Vec<ThreadJoinHandle<()>>),
    ) {
        let regional = matches!(target.group, GroupId::MemoryRegion(_));
        let known = !regional || self.membership(target.processor).load(Ordering::Acquire);
        // Keep task ownership outside all guards so rejection never drops captures under a lock.
        let mut task = Some(task);
        let (accepted, expanded) = if known && state.startup.get().is_some() {
            self.admit(target, state, &mut task, urgent, None)
        } else {
            // Only first use of a group/CPU takes this lock. It serializes expansion with
            // shutdown and initial readiness; no other submitter can overtake first admission.
            let mut handles = self.worker_handles.lock().expect(NEVER_POISONED);
            let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                assert!(!self.poisoned.load(Ordering::Acquire));
                if self.shutdown.load(Ordering::Acquire) {
                    return (false, false);
                }
                if state.startup.get().is_none() {
                    let result =
                        panic::catch_unwind(panic::AssertUnwindSafe(|| start(&mut handles)));
                    state
                        .startup
                        .set(if result.is_ok() { Ok(()) } else { Err(()) })
                        .expect("startup is published once under the worker handle lock");
                    if let Err(payload) = result {
                        panic::resume_unwind(payload);
                    }
                }
                let expanded =
                    regional && !self.membership(target.processor).load(Ordering::Acquire);
                let affinity = expanded.then(|| {
                    Arc::new(
                        self.hardware
                            .all_processors()
                            .to_builder()
                            .filter(|processor| {
                                target.group.contains(processor)
                                    && (processor.id() == target.processor
                                        || self.membership(processor.id()).load(Ordering::Acquire))
                            })
                            .take_all()
                            .expect("the sampled submitting processor belongs to its region"),
                    )
                });
                self.admit(target, state, &mut task, urgent, affinity)
            }));
            if result.is_err() {
                self.poisoned.store(true, Ordering::Release);
            }
            drop(handles);
            match result {
                Ok(accepted) => accepted,
                Err(payload) => self.poison(payload),
            }
        };
        if expanded {
            state.wake_event.notify(usize::MAX);
        }
        if accepted {
            state.record_task_spawned();
            // Each queued task may block its worker; do not coalesce wakeups.
            state.wake_event.notify(1.additional());
            trace!(pool_id = self.pool_id, group = ?target.group, urgent, "spawned task");
        } else {
            trace!(pool_id = self.pool_id, group = ?target.group, "abandoned task during shutdown");
        }
    }

    /// Pins every new worker to the first submitter before admitting any work.
    #[cfg_attr(test, mutants::skip)] // Thread/acknowledgment mutations can block startup indefinitely.
    pub(crate) fn start_workers(
        self: &Arc<Self>,
        target: SubmissionTarget,
        handles: &mut Vec<ThreadJoinHandle<()>>,
        mut spawn: impl FnMut(thread::Builder, WorkerEntry) -> io::Result<ThreadJoinHandle<()>>,
        pin: impl Fn(&ProcessorSet) + Clone + Send + 'static,
    ) {
        let destination = target.group;
        let affinity = self
            .hardware
            .all_processors()
            .to_builder()
            .filter(|processor| {
                processor.id() == target.processor && destination.contains(processor)
            })
            .take_all()
            .unwrap_or_else(|| panic!("no eligible processors for worker group {destination:?}"));
        let affinity = Arc::new(affinity);

        for worker_index in 0..self.policy.count().get() {
            // One readiness message per worker; no task is admitted before all acknowledge.
            let (ready, receiver) = mpsc::sync_channel(1);
            let entry: WorkerEntry = Box::new({
                let inner = Arc::clone(self);
                let affinity = Arc::clone(&affinity);
                let pin = pin.clone();
                move || {
                    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| pin(&affinity)));
                    let pinned = result.is_ok();
                    if ready.send(result).is_err() || !pinned {
                        return;
                    }
                    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                        debug!(
                            pool_name = inner.pool_name.as_str(),
                            pool_id = inner.pool_id,
                            group = ?destination,
                            worker_index,
                            "worker thread started"
                        );
                        worker_loop(&inner, destination, worker_index, &pin);
                        debug!(
                            pool_name = inner.pool_name.as_str(),
                            pool_id = inner.pool_id,
                            group = ?destination,
                            worker_index,
                            "worker thread exiting"
                        );
                    }));
                    if let Err(payload) = result {
                        inner.poison(payload);
                    }
                }
            });
            let builder = thread::Builder::new()
                .name(format!("{}-{destination}-{worker_index}", self.pool_name));
            let handle = spawn(builder, entry).unwrap_or_else(|error| {
                panic!("failed to start worker in {destination:?}: {error}")
            });
            handles.push(handle);
            match receiver.recv() {
                Ok(Ok(())) => {}
                Ok(Err(payload)) => panic::resume_unwind(payload),
                Err(error) => panic!("worker in {destination:?} exited before readiness: {error}"),
            }
        }
    }

    /// Publishes membership only with an accepted task, under its queue's admission lock.
    #[cfg_attr(test, mutants::skip)] // Accepting after shutdown/failure strands joined handles.
    fn admit(
        &self,
        target: SubmissionTarget,
        state: &WorkerGroup,
        task: &mut Option<ErasedTaskHandle>,
        urgent: bool,
        affinity: Option<Arc<ProcessorSet>>,
    ) -> (bool, bool) {
        let queue = if urgent {
            &state.urgent_queue
        } else {
            &state.regular_queue
        };
        let mut queue = queue.lock().expect(NEVER_POISONED);
        if self.poisoned.load(Ordering::Acquire) {
            drop(queue);
            panic!("worker pool is poisoned");
        }
        if self.shutdown.load(Ordering::Acquire) {
            return (false, false);
        }
        queue.push_back(task.take().expect("admission consumes each task once"));
        let expanded = affinity.is_some();
        if let Some(affinity) = affinity {
            let mut current = state.affinity.lock().expect(NEVER_POISONED);
            *current = Some(affinity);
            // Revisions cannot overflow: each CPU enters this fixed topology at most once.
            state.affinity_revision.fetch_add(1, Ordering::Release);
            self.membership(target.processor)
                .store(true, Ordering::Release);
        }
        drop(queue);
        (true, expanded)
    }

    /// Resolves CPU membership in the topology's sparse index space.
    fn membership(&self, processor: ProcessorId) -> &AtomicBool {
        self.submitted_processors
            .get(processor as usize)
            .expect("a topology processor must fit its index-space bound")
    }

    /// Stops every group without joining the reporting worker or dropping captures under locks.
    #[cfg_attr(test, mutants::skip)] // Missing terminal cleanup can strand workers and joined handles.
    fn poison(&self, payload: Box<dyn Any + Send>) -> ! {
        self.poisoned.store(true, Ordering::Release);
        self.shutdown.store(true, Ordering::Release);
        error!(
            pool_id = self.pool_id,
            message = extract_panic_message(&payload),
            "worker pool infrastructure failed"
        );
        // Wait for any in-progress startup to register all handles before scanning groups.
        // Handles stay registered: a failing worker cannot join itself.
        drop(self.worker_handles.lock().expect(NEVER_POISONED));
        for state in self.registry.initialized() {
            state.signal_shutdown();
        }
        let mut failure = Err(payload);
        for state in self.registry.initialized() {
            discard_queue(&state.urgent_queue, &mut failure, &self.poisoned);
            discard_queue(&state.regular_queue, &mut failure, &self.poisoned);
        }
        panic::resume_unwind(failure.expect_err("pool failure retains its original panic"));
    }

    /// Stops admission and releases abandoned tasks even when schedulers retain the storage.
    #[cfg_attr(test, mutants::skip)] // Removing shutdown/join can leave sleeping workers forever.
    pub(crate) fn join_all_workers(&self) {
        let handles = {
            let mut handles = self.worker_handles.lock().expect(NEVER_POISONED);
            self.shutdown.store(true, Ordering::Release);
            // No new handles can be registered after this terminal transition. Reentrant
            // submissions cannot add work, so the detached list cannot become stale.
            mem::take(&mut *handles)
        };
        for state in self.registry.initialized() {
            state.signal_shutdown();
        }

        let mut failure = Ok(());
        for state in self.registry.initialized() {
            discard_queue(&state.urgent_queue, &mut failure, &self.poisoned);
            discard_queue(&state.regular_queue, &mut failure, &self.poisoned);
        }
        for handle in handles {
            record_failure(&mut failure, handle.join(), &self.poisoned);
        }
        if let Err(payload) = failure {
            if thread::panicking() {
                error!(
                    pool_id = self.pool_id,
                    "worker or task cleanup panicked during unwinding"
                );
            } else {
                panic::resume_unwind(payload);
            }
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for PoolInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("pool_id", &self.pool_id)
            .field("pool_name", &self.pool_name)
            .field("policy", &self.policy)
            .finish_non_exhaustive()
    }
}

/// The standard thread adapter and failure-injection tests share this entry-point boundary.
type WorkerEntry = Box<dyn FnOnce() + Send>;

/// Drains the live queue without running user destructors under a lock.
#[cfg_attr(test, mutants::skip)] // Skipping cleanup leaves abandoned receivers pending.
fn discard_queue(
    queue: &Mutex<VecDeque<ErasedTaskHandle>>,
    failure: &mut ThreadResult<()>,
    poisoned: &AtomicBool,
) {
    loop {
        let task = queue.lock().expect(NEVER_POISONED).pop_front();
        let Some(task) = task else { break };
        record_failure(
            failure,
            panic::catch_unwind(panic::AssertUnwindSafe(|| drop(task))),
            poisoned,
        );
    }
}

/// Preserves the first failure while ensuring all remaining resources are cleaned up.
fn record_failure(first: &mut ThreadResult<()>, result: ThreadResult<()>, poisoned: &AtomicBool) {
    if let Err(payload) = result {
        poisoned.store(true, Ordering::Release);
        error!(
            message = extract_panic_message(&payload),
            "failure during worker pool cleanup"
        );
        if first.is_ok() {
            *first = Err(payload);
        } else {
            error!("additional panic while shutting down worker pool");
        }
    }
}

/// Drives the unchanged priority executor with this group's queues and wake signal.
#[cfg_attr(test, mutants::skip)] // Broken wakeup coordination can strand workers indefinitely.
fn worker_loop(
    inner: &PoolInner,
    destination: GroupId,
    worker_index: u32,
    pin: impl Fn(&ProcessorSet),
) {
    let state = inner.registry.get_or_init(destination);
    let core = WorkerCore::new(
        &state.urgent_queue,
        &state.regular_queue,
        &state.shutdown_flag,
    );
    let mut revision = 0;
    let regional = matches!(destination, GroupId::MemoryRegion(_));
    loop {
        if inner.shutdown.load(Ordering::Acquire) {
            break;
        }
        if regional {
            refresh_affinity(state, &mut revision, &pin);
        }
        match core.run_one_iteration_with(|| {
            if inner.shutdown.load(Ordering::Acquire) {
                return false;
            }
            if regional {
                refresh_affinity(state, &mut revision, &pin);
            }
            !inner.shutdown.load(Ordering::Acquire)
        }) {
            IterationResult::ExecutedUrgent | IterationResult::ExecutedRegular => {
                trace!(pool_id = inner.pool_id, group = ?destination, worker_index, "executed task");
            }

            IterationResult::Shutdown => break,
            IterationResult::WaitingForWork => {
                listener!(state.wake_event => listener);
                if !state.urgent_queue.lock().expect(NEVER_POISONED).is_empty()
                    || !state.regular_queue.lock().expect(NEVER_POISONED).is_empty()
                    || state.shutdown_flag.load(Ordering::Acquire)
                    || state.affinity_revision.load(Ordering::Acquire) != revision
                {
                    continue;
                }
                listener.wait();
            }
        }
    }
}

/// Applies a monotonically published snapshot on the owning worker, never under a shared lock.
#[cfg_attr(test, mutants::skip)] // Missed revisions can strand idle workers or accepted work.
pub(crate) fn refresh_affinity(
    state: &WorkerGroup,
    revision: &mut usize,
    pin: impl FnOnce(&ProcessorSet),
) {
    if state.affinity_revision.load(Ordering::Acquire) == *revision {
        return;
    }
    let (affinity, latest) = {
        let affinity = state.affinity.lock().expect(NEVER_POISONED);
        (
            Arc::clone(
                affinity
                    .as_ref()
                    .expect("a revision publishes a nonempty mask"),
            ),
            state.affinity_revision.load(Ordering::Acquire),
        )
    };
    pin(&affinity);
    *revision = latest;
}
