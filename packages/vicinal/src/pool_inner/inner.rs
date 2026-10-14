//! Shared pool ownership, worker startup and terminal cleanup.

use std::any::{Any, type_name};
use std::collections::VecDeque;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle as ThreadJoinHandle, Result as ThreadResult};
use std::{fmt, io, mem};

use many_cpus::{ProcessorId, ProcessorSet, SystemHardware};
use tracing::{debug, error};

use crate::pool_inner::worker_loop;
use crate::{
    ErasedTaskHandle, GroupId, GroupRegistry, NEVER_POISONED, SubmissionTarget, ThreadInit,
    WorkerGroup, WorkerPolicy, discard_panic, extract_panic_message,
};

/// Groups pool lifecycle diagnostics under their owner, including admission and execution.
pub(crate) const POOL_LOG_TARGET: &str = module_path!();

/// Keeps schedulers' shared storage alive while the owning pool controls worker lifetime.
pub(crate) struct PoolInner {
    pub(crate) pool_id: u64,
    pub(crate) pool_name: String,
    pub(crate) hardware: SystemHardware,
    pub(crate) registry: GroupRegistry,
    pub(crate) policy: WorkerPolicy,
    thread_init: Option<ThreadInit>,

    /// Serializes cold startup and handle registration with the shutdown transition.
    pub(crate) worker_handles: Mutex<Vec<ThreadJoinHandle<()>>>,

    pub(crate) shutdown: AtomicBool,
    /// Infrastructure failure is permanent and distinct from clean shutdown abandonment.
    pub(crate) poisoned: AtomicBool,

    /// Initial selections and accepted submitters share membership, independent of ID gaps.
    pub(crate) member_processors: Box<[(ProcessorId, AtomicBool)]>,
}

impl PoolInner {
    pub(crate) fn new(
        pool_id: u64,
        pool_name: String,
        hardware: SystemHardware,
        policy: WorkerPolicy,
        thread_init: Option<ThreadInit>,
    ) -> Self {
        let member_processors = if policy.shares_processors() {
            let mut processors = hardware
                .all_processors()
                .iter()
                .map(|processor| (processor.id(), AtomicBool::new(false)))
                .collect::<Vec<_>>();
            processors.sort_unstable_by_key(|(id, _)| *id);

            processors.into_boxed_slice()
        } else {
            Box::default()
        };

        Self {
            pool_id,
            pool_name,
            registry: GroupRegistry::new(&hardware, &policy),
            hardware,
            policy,
            thread_init,
            worker_handles: Mutex::new(Vec::new()),
            shutdown: AtomicBool::new(false),
            poisoned: AtomicBool::new(false),
            member_processors,
        }
    }

    /// Publishes the affinity-startup outcome while the caller serializes worker registration.
    #[cfg_attr(test, mutants::skip)] // Missing startup publication can start duplicate workers.
    pub(crate) fn start_group<R>(state: &WorkerGroup, start: impl FnOnce() -> R) -> R {
        let result = panic::catch_unwind(AssertUnwindSafe(start));
        state
            .startup
            .set(if result.is_ok() { Ok(()) } else { Err(()) })
            .expect("startup is published once under the worker handle lock");

        result.unwrap_or_else(|payload| panic::resume_unwind(payload))
    }

    /// Pins every new worker to the first submitter before admitting any work.
    // Thread/acknowledgment mutations can block startup indefinitely.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn start_workers(
        self: &Arc<Self>,
        target: SubmissionTarget,
        handles: &mut Vec<ThreadJoinHandle<()>>,
        spawn: impl FnMut(thread::Builder, WorkerEntry) -> io::Result<ThreadJoinHandle<()>>,
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

        // Lazy admission waits only for affinity; completion receivers are needed by eager builds.
        _ = self.start_workers_on(destination, &affinity, handles, spawn, pin);
    }

    /// Shares worker entry and affinity readiness between lazy and eager group startup.
    ///
    /// Each returned receiver acknowledges initializer completion after its affinity message
    /// has been consumed here. Disconnection leaves the worker's panic in its registered handle.
    #[cfg_attr(test, mutants::skip)] // Broken acknowledgments or registration can strand workers.
    pub(crate) fn start_workers_on(
        self: &Arc<Self>,
        destination: GroupId,
        affinity: &Arc<ProcessorSet>,
        handles: &mut Vec<ThreadJoinHandle<()>>,
        mut spawn: impl FnMut(thread::Builder, WorkerEntry) -> io::Result<ThreadJoinHandle<()>>,
        pin: impl Fn(&ProcessorSet) + Clone + Send + 'static,
    ) -> Vec<mpsc::Receiver<ThreadResult<()>>> {
        // Startup owns registration, so this revision describes the supplied initial mask.
        // Acquiring pairs with eager snapshot publication before threads are spawned.
        let revision = self
            .registry
            .get_or_init(destination)
            .affinity_revision
            .load(Ordering::Acquire);
        let mut initialized = Vec::new();

        for worker_index in 0..self.policy.count().get() {
            // The buffer permits initialization to finish without waiting for the constructor.
            // Affinity and initialization share the channel, in that order.
            let (ready, receiver) = mpsc::sync_channel(1);
            let entry: WorkerEntry = Box::new({
                let inner = Arc::clone(self);
                let affinity = Arc::clone(affinity);
                let pin = pin.clone();
                move || {
                    let result = panic::catch_unwind(AssertUnwindSafe(|| pin(&affinity)));
                    let pinned = result.is_ok();
                    if ready.send(result).is_err() || !pinned {
                        return;
                    }

                    inner.run_worker(destination, worker_index, revision, ready, &pin);
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
            initialized.push(receiver);
        }

        initialized
    }

    /// Contains initialization and the task loop within the worker's infrastructure boundary.
    // Omitting initialization or failure propagation can strand accepted work.
    #[cfg_attr(test, mutants::skip)]
    fn run_worker(
        &self,
        destination: GroupId,
        worker_index: u32,
        revision: usize,
        ready: mpsc::SyncSender<ThreadResult<()>>,
        pin: impl Fn(&ProcessorSet),
    ) {
        let result = panic::catch_unwind(AssertUnwindSafe(|| {
            debug!(
                pool_name = self.pool_name.as_str(),
                pool_id = self.pool_id,
                group = ?destination,
                worker_index,
                "worker thread started"
            );

            // Affinity is already acknowledged, so initialization can submit work without
            // waiting on its own startup transaction. See docs/implementation.md.
            if let Some(thread_init) = &self.thread_init {
                (thread_init.0)();
            }

            _ = ready.send(Ok(()));
            drop(ready);
            worker_loop(self, destination, worker_index, revision, &pin);

            debug!(
                pool_name = self.pool_name.as_str(),
                pool_id = self.pool_id,
                group = ?destination,
                worker_index,
                "worker thread exiting"
            );
        }));

        if let Err(payload) = result {
            self.poison(payload);
        }
    }

    /// Publishes failure before cleanup can invoke reentrant task destructors, without callbacks.
    // Mutating terminal state can admit work to workers that are exiting after failure.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn stop_admission(&self) {
        self.poisoned.store(true, Ordering::Release);
        self.shutdown.store(true, Ordering::Release);
    }

    /// Stops every group without joining the reporting worker or dropping captures under locks.
    // Missing terminal cleanup can strand workers and joined handles.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn poison(&self, payload: Box<dyn Any + Send>) -> ! {
        self.stop_admission();
        error!(
            pool_id = self.pool_id,
            message = extract_panic_message(&payload),
            "worker pool infrastructure failed"
        );

        // Wait for any in-progress startup to register all handles before scanning groups.
        // Handles stay registered: a failing worker cannot join itself.
        drop(self.worker_handles.lock().expect(NEVER_POISONED));

        let mut failure = Err(payload);
        self.stop_groups(&mut failure);

        panic::resume_unwind(failure.expect_err("pool failure retains its original panic"));
    }

    /// Stops admission and releases abandoned tasks even when schedulers retain the storage.
    // Removing shutdown/join can leave sleeping workers forever.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn join_all_workers(&self) {
        let handles = {
            let mut handles = self.worker_handles.lock().expect(NEVER_POISONED);
            self.shutdown.store(true, Ordering::Release);
            // No new handles can be registered after this terminal transition. Reentrant
            // submissions cannot add work, so the detached list cannot become stale.
            mem::take(&mut *handles)
        };

        let mut failure = Ok(());
        self.stop_groups(&mut failure);

        for handle in handles {
            record_failure(&mut failure, handle.join(), &self.poisoned);
        }

        if let Err(payload) = failure {
            if thread::panicking() {
                error!(
                    pool_id = self.pool_id,
                    "worker or task cleanup panicked during unwinding"
                );
                discard_panic(payload);
            } else {
                panic::resume_unwind(payload);
            }
        }
    }

    /// Wakes every group before capture destruction can block or re-enter the stopped pool.
    // Missing terminal cleanup can strand workers and joined handles.
    #[cfg_attr(test, mutants::skip)]
    fn stop_groups(&self, failure: &mut ThreadResult<()>) {
        for state in self.registry.initialized() {
            state.signal_shutdown();
        }

        for state in self.registry.initialized() {
            discard_queue(&state.urgent_queue, failure, &self.poisoned);
            discard_queue(&state.regular_queue, failure, &self.poisoned);
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
pub(crate) type WorkerEntry = Box<dyn FnOnce() + Send>;

/// Drains the live queue without running user destructors under a lock.
// Skipping cleanup leaves abandoned receivers pending.
#[cfg_attr(test, mutants::skip)]
fn discard_queue(
    queue: &Mutex<VecDeque<ErasedTaskHandle>>,
    failure: &mut ThreadResult<()>,
    poisoned: &AtomicBool,
) {
    loop {
        let task = queue.lock().expect(NEVER_POISONED).pop_front();
        let Some(task) = task else {
            break;
        };

        record_failure(
            failure,
            panic::catch_unwind(AssertUnwindSafe(|| drop(task))),
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
            discard_panic(payload);
        }
    }
}
