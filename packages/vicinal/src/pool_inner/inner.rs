//! Cold worker startup, shared admission and shutdown coordination.

use std::any::type_name;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle as ThreadJoinHandle, Result as ThreadResult};
use std::{fmt, io, mem, panic};

use event_listener::{IntoNotification, Listener, listener};
use many_cpus::{ProcessorSet, SystemHardware};
use tracing::{debug, error, trace};

use crate::{
    ErasedTaskHandle, GroupId, GroupRegistry, IterationResult, NEVER_POISONED, WorkerCore,
    WorkerGroup, WorkerPolicy,
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
}

impl PoolInner {
    pub(crate) fn new(
        pool_id: u64,
        pool_name: String,
        hardware: SystemHardware,
        policy: WorkerPolicy,
    ) -> Self {
        Self {
            pool_id,
            pool_name,
            registry: GroupRegistry::new(&hardware, &policy),
            hardware,
            policy,
            worker_handles: Mutex::new(Vec::new()),
            shutdown: AtomicBool::new(false),
        }
    }

    /// Returns the sampled group's storage only after its complete worker count is ready.
    pub(crate) fn ensure_workers_spawned(self: &Arc<Self>, destination: GroupId) -> &WorkerGroup {
        self.ensure_workers_spawned_with(destination, |handles| {
            self.start_workers(
                destination,
                handles,
                thread::Builder::spawn,
                ProcessorSet::pin_current_thread_to,
            );
        })
    }

    /// The startup transaction is separate from the OS adapter so failures can be exercised
    /// without relying on thread exhaustion or changing the host's affinity restrictions.
    #[cfg_attr(test, mutants::skip)] // Broken startup/registration can strand workers or submissions.
    pub(crate) fn ensure_workers_spawned_with(
        self: &Arc<Self>,
        destination: GroupId,
        start: impl FnOnce(&mut Vec<ThreadJoinHandle<()>>),
    ) -> &WorkerGroup {
        let state = self.registry.get_or_init(destination);
        if let Some(result) = state.startup.get() {
            assert!(
                result.is_ok(),
                "worker group {destination:?} failed to start"
            );
            return state;
        }

        let mut handles = self.worker_handles.lock().expect(NEVER_POISONED);
        if self.shutdown.load(Ordering::Acquire) {
            return state;
        }
        if let Some(result) = state.startup.get() {
            drop(handles);
            assert!(
                result.is_ok(),
                "worker group {destination:?} failed to start"
            );
            return state;
        }

        // Every created handle is registered before readiness is acknowledged. Catch only
        // startup failures here, publish a terminal state, then propagate outside the lock.
        // Ref: docs/implementation.md, "Queue and worker lifecycle".
        let result = panic::catch_unwind(panic::AssertUnwindSafe(|| start(&mut handles)));
        state
            .startup
            .set(if result.is_ok() { Ok(()) } else { Err(()) })
            .expect("startup is published once under the worker handle lock");
        drop(handles);

        if let Err(payload) = result {
            state.signal_shutdown();
            panic::resume_unwind(payload);
        }
        state
    }

    /// Starts named workers using one cached eligible affinity set for the group.
    #[cfg_attr(test, mutants::skip)] // Thread/acknowledgment mutations can block startup indefinitely.
    pub(crate) fn start_workers(
        self: &Arc<Self>,
        destination: GroupId,
        handles: &mut Vec<ThreadJoinHandle<()>>,
        mut spawn: impl FnMut(thread::Builder, WorkerEntry) -> io::Result<ThreadJoinHandle<()>>,
        pin: fn(&ProcessorSet),
    ) {
        let affinity = self
            .hardware
            .all_processors()
            .to_builder()
            .filter(|processor| destination.contains(processor))
            .take_all()
            .unwrap_or_else(|| panic!("no eligible processors for worker group {destination:?}"));
        let affinity = Arc::new(affinity);

        for worker_index in 0..self.policy.count().get() {
            // One readiness message per worker; no task is admitted before all acknowledge.
            let (ready, receiver) = mpsc::sync_channel(1);
            let entry: WorkerEntry = Box::new({
                let inner = Arc::clone(self);
                let affinity = Arc::clone(&affinity);
                move || {
                    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| pin(&affinity)));
                    let pinned = result.is_ok();
                    if ready.send(result).is_err() || !pinned {
                        return;
                    }
                    debug!(
                        pool_name = inner.pool_name.as_str(),
                        pool_id = inner.pool_id,
                        group = ?destination,
                        worker_index,
                        "worker thread started"
                    );
                    worker_loop(&inner, destination, worker_index);
                    debug!(
                        pool_name = inner.pool_name.as_str(),
                        pool_id = inner.pool_id,
                        group = ?destination,
                        worker_index,
                        "worker thread exiting"
                    );
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

    /// Makes enqueue and shutdown draining mutually exclusive at the destination queue.
    #[cfg_attr(test, mutants::skip)] // Lost enqueue/notification can strand a waiting join handle.
    pub(crate) fn enqueue(
        &self,
        destination: GroupId,
        state: &WorkerGroup,
        task: ErasedTaskHandle,
        urgent: bool,
    ) {
        let queue = if urgent {
            &state.urgent_queue
        } else {
            &state.regular_queue
        };
        let mut queue = queue.lock().expect(NEVER_POISONED);
        if self.shutdown.load(Ordering::Acquire) {
            drop(queue);
            trace!(pool_id = self.pool_id, group = ?destination, "abandoned task during shutdown");
            // Captures and result senders may call arbitrary user code when dropped.
            drop(task);
            return;
        }
        queue.push_back(task);
        drop(queue);
        state.record_task_spawned();
        // Each queued task can occupy a worker indefinitely. Wake an additional listener
        // rather than coalescing notifications while an earlier worker is still waking.
        state.wake_event.notify(1.additional());
        trace!(pool_id = self.pool_id, group = ?destination, urgent, "spawned task");
    }

    /// Stops admission and releases abandoned tasks even when schedulers retain the storage.
    #[cfg_attr(test, mutants::skip)] // Removing shutdown/join can leave sleeping workers forever.
    pub(crate) fn join_all_workers(&self) {
        let handles = {
            let mut handles = self.worker_handles.lock().expect(NEVER_POISONED);
            self.shutdown.store(true, Ordering::Release);
            // No new handles can be registered after this terminal transition. Reentrant
            // submissions can only abandon work, so the detached list cannot become stale.
            mem::take(&mut *handles)
        };
        for state in self.registry.initialized() {
            state.signal_shutdown();
        }

        let mut failure = Ok(());
        for state in self.registry.initialized() {
            discard_queue(&state.urgent_queue, &mut failure);
            discard_queue(&state.regular_queue, &mut failure);
        }
        for handle in handles {
            record_failure(&mut failure, handle.join());
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
fn discard_queue(queue: &Mutex<VecDeque<ErasedTaskHandle>>, failure: &mut ThreadResult<()>) {
    loop {
        let task = queue.lock().expect(NEVER_POISONED).pop_front();
        let Some(task) = task else { break };
        record_failure(
            failure,
            panic::catch_unwind(panic::AssertUnwindSafe(|| drop(task))),
        );
    }
}

/// Preserves the first failure while ensuring all remaining resources are cleaned up.
fn record_failure(first: &mut ThreadResult<()>, result: ThreadResult<()>) {
    if let Err(payload) = result {
        if first.is_ok() {
            *first = Err(payload);
        } else {
            error!("additional panic while shutting down worker pool");
        }
    }
}

/// Drives the unchanged priority executor with this group's queues and wake signal.
#[cfg_attr(test, mutants::skip)] // Broken wakeup coordination can strand workers indefinitely.
fn worker_loop(inner: &PoolInner, destination: GroupId, worker_index: u32) {
    let state = inner.registry.get_or_init(destination);
    let core = WorkerCore::new(
        &state.urgent_queue,
        &state.regular_queue,
        &state.shutdown_flag,
    );
    loop {
        match core.run_one_iteration() {
            IterationResult::ExecutedUrgent | IterationResult::ExecutedRegular => {
                trace!(pool_id = inner.pool_id, group = ?destination, worker_index, "executed task");
            }

            IterationResult::Shutdown => break,
            IterationResult::WaitingForWork => {
                listener!(state.wake_event => listener);
                if !state.urgent_queue.lock().expect(NEVER_POISONED).is_empty()
                    || !state.regular_queue.lock().expect(NEVER_POISONED).is_empty()
                    || state.shutdown_flag.load(Ordering::Acquire)
                {
                    continue;
                }
                listener.wait();
            }
        }
    }
}
