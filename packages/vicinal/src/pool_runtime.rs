//! Pool-wide terminal state shared by every policy-created worker group.

use std::any::Any;
use std::collections::VecDeque;
use std::mem::replace;
use std::ops::ControlFlow;
#[cfg(test)]
use std::panic::panic_any;
use std::panic::{AssertUnwindSafe, catch_unwind};
#[cfg(test)]
use std::panic::{RefUnwindSafe, UnwindSafe};
#[cfg(test)]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(test)]
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::Result as ThreadResult;

use events_once::EventLake;
use tracing::error;

use crate::pool_inner::POOL_LOG_TARGET;
use crate::{
    ErasedTaskHandle, NEVER_POISONED, PanicFailures, ThreadInit, WorkerGroupState,
    extract_panic_message,
};

/// Coordinates terminal state, group registration, failure ownership and worker participation.
///
/// It is shared by the pool, schedulers and every policy-created worker. Ref:
/// docs/implementation.md, "Ownership and coordination" and "Affinity and failure coordination".
pub(crate) struct PoolRuntime {
    pool_id: u64,
    pool_name: Arc<str>,
    // Failed construction must release the callback on the builder, even while
    // a departed policy thread still owns a runtime handle.
    thread_init: Mutex<Option<Arc<ThreadInit>>>,
    shutdown: AtomicBool,
    poisoned: AtomicBool,
    groups: Mutex<Vec<RegisteredGroup>>,
    workers: Mutex<WorkerLifecycle>,
    workers_changed: Condvar,
    #[cfg(test)]
    pub(crate) before_worker_wait:
        OnceLock<Box<dyn Fn() + Send + Sync + UnwindSafe + RefUnwindSafe>>,
    // Signals the fixture while a live reservation still holds the builder at the wait.
    #[cfg(test)]
    pub(crate) before_binding_wait: OnceLock<Sender<()>>,
    failure: Mutex<FailureState>,
    pub(crate) result_channel_pool: EventLake,
}

impl PoolRuntime {
    pub(crate) fn new(
        pool_id: u64,
        pool_name: String,
        thread_init: Option<ThreadInit>,
    ) -> Arc<Self> {
        Arc::new(Self {
            pool_id,
            pool_name: Arc::from(pool_name),
            thread_init: Mutex::new(thread_init.map(Arc::new)),
            shutdown: AtomicBool::new(false),
            poisoned: AtomicBool::new(false),
            groups: Mutex::new(Vec::new()),
            workers: Mutex::new(WorkerLifecycle::default()),
            workers_changed: Condvar::new(),
            #[cfg(test)]
            before_worker_wait: OnceLock::new(),
            #[cfg(test)]
            before_binding_wait: OnceLock::new(),
            failure: Mutex::new(FailureState::Vacant),
            result_channel_pool: EventLake::new(),
        })
    }

    pub(crate) fn pool_id(&self) -> u64 {
        self.pool_id
    }

    pub(crate) fn pool_name(&self) -> &str {
        &self.pool_name
    }

    /// Gives an initializing worker a callback reference without holding the slot lock.
    pub(crate) fn thread_init(&self) -> Option<Arc<ThreadInit>> {
        self.thread_init.lock().expect(NEVER_POISONED).clone()
    }

    /// Releases the initializer on the failed builder after participating workers have left.
    pub(crate) fn release_thread_init(&self) {
        let callback = self.thread_init.lock().expect(NEVER_POISONED).take();
        drop(callback);
    }

    pub(crate) fn is_shutdown(&self) -> bool {
        // Acquisition observes terminal state published by stop_workers.
        self.shutdown.load(Ordering::Acquire)
    }

    pub(crate) fn is_poisoned(&self) -> bool {
        // Poison is only an admission/terminal signal; failure payload ownership is separately
        // protected by the failure mutex and FailureState.
        self.poisoned.load(Ordering::Acquire)
    }

    // Omitting terminal registration strands participating workers outside shutdown scans.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn register_group(&self, group: &Arc<WorkerGroupState>) {
        let mut groups = self.groups.lock().expect(NEVER_POISONED);
        if self.is_poisoned() {
            drop(groups);
            panic!("worker groups cannot be added after pool failure");
        }

        groups.push(RegisteredGroup {
            state: Arc::clone(group),
        });
        if self.is_shutdown() {
            group.signal_shutdown();
        }
    }

    pub(crate) fn begin_worker(&self) -> ControlFlow<()> {
        // The worker mutex synchronizes participation with the shutdown waiter. Checking
        // shutdown while holding it prevents a worker from entering after the last wait ends.
        let mut workers = self.workers.lock().expect(NEVER_POISONED);
        if self.is_shutdown() {
            return ControlFlow::Break(());
        }

        workers.active = workers.active.checked_add(1).expect(
            "the process cannot allocate more participating workers than addressable memory",
        );
        ControlFlow::Continue(())
    }

    /// Enables builder-owned tracking for workers reserved by policy binding.
    pub(crate) fn begin_worker_binding(&self) {
        let mut workers = self.workers.lock().expect(NEVER_POISONED);
        assert!(
            !workers.binding,
            "policy binding begins only once per pool runtime"
        );
        workers.binding = true;
    }

    /// Returns whether a worker belongs to the current policy-binding phase.
    pub(crate) fn reserve_binding_worker(&self) -> bool {
        let mut workers = self.workers.lock().expect(NEVER_POISONED);
        if !workers.binding {
            return false;
        }

        workers.binding_workers = workers
            .binding_workers
            .checked_add(1)
            .expect("reserved workers cannot exceed the process's addressable handle capacity");
        true
    }

    /// Releases the builder's readiness target after initialization or cancellation.
    // Omitting this decrement and notification can leave binding waiting forever.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn complete_binding_worker(&self) {
        let mut workers = self.workers.lock().expect(NEVER_POISONED);
        workers.binding_workers = workers
            .binding_workers
            .checked_sub(1)
            .expect("completing a binding reservation requires one outstanding reservation");
        self.workers_changed.notify_all();
    }

    /// Stops assigning newly reserved workers to the completed binding phase.
    pub(crate) fn end_worker_binding(&self) {
        self.workers.lock().expect(NEVER_POISONED).binding = false;
    }

    /// Waits until policy-binding workers initialize or the pool fails.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn wait_for_binding_workers(&self) {
        let mut workers = self.workers.lock().expect(NEVER_POISONED);
        while workers.binding_workers != 0 && !self.is_poisoned() {
            #[cfg(test)]
            if let Some(waiting) = self.before_binding_wait.get() {
                // Holding the mutex ensures cancellation cannot overtake the wait.
                _ = waiting.send(());
            }
            workers = self.workers_changed.wait(workers).expect(NEVER_POISONED);
        }
    }

    // Omitting this decrement can leave the shutdown barrier waiting forever.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn end_worker(&self) {
        // Releases the shutdown participation barrier; every successful begin has one end.
        // The notification can satisfy wait_for_workers only after failure cleanup completed.
        let mut workers = self.workers.lock().expect(NEVER_POISONED);
        workers.active = workers
            .active
            .checked_sub(1)
            .expect("end_worker pairs exactly with one successful begin_worker call");
        self.workers_changed.notify_all();
    }

    #[cfg_attr(test, mutants::skip)]
    /// Publishes pool shutdown and wakes every registered group to leave its worker loop.
    pub(crate) fn stop_workers(&self) {
        // Release publishes the pool-wide terminal transition before the per-group wakeups.
        let groups = self.groups.lock().expect(NEVER_POISONED);
        self.shutdown.store(true, Ordering::Release);

        for group in groups.iter() {
            group.state.signal_shutdown();
            group.state.notify_initialization_failure();
            #[cfg(test)]
            group
                .state
                .mark_startup_failed_if_started(self.is_poisoned());
        }
        self.workers_changed.notify_all();
    }

    pub(crate) fn stop_admission(&self) {
        // Release/Acquire publishes only permanent rejection. Failure payload ownership uses
        // the mutex-protected FailureState independently.
        self.poisoned.store(true, Ordering::Release);
        self.stop_workers();
    }

    // A false predicate can wait forever after the last worker has already exited.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn wait_for_workers(&self) {
        // This is the pool-owned barrier, not a join of policy-owned OS threads.
        let mut workers = self.workers.lock().expect(NEVER_POISONED);
        while workers.active != 0 {
            #[cfg(test)]
            if let Some(before_wait) = self.before_worker_wait.get() {
                before_wait();
            }
            workers = self.workers_changed.wait(workers).expect(NEVER_POISONED);
        }
    }

    #[cfg(test)]
    pub(crate) fn active_workers(&self) -> usize {
        self.workers.lock().expect(NEVER_POISONED).active
    }

    #[cfg(test)]
    pub(crate) fn publish_shutdown_without_group_signals(&self) {
        self.shutdown.store(true, Ordering::Release);
    }

    /// Retains worker failures and replaces the primary value with later incidental panics.
    ///
    /// Ref: docs/implementation.md, "Affinity and failure coordination".
    pub(crate) fn fail(&self, failures: PanicFailures) {
        let message = {
            let mut failure = self.failure.lock().expect(NEVER_POISONED);
            match &mut *failure {
                FailureState::Vacant => {
                    let message = extract_panic_message(failures.latest());
                    *failure = FailureState::Retained(failures);
                    message
                }
                FailureState::Retained(retained) => {
                    retained.append(failures);
                    return;
                }
                FailureState::Unavailable => {
                    drop(failure);
                    // Another boundary already took ownership of the published failure.
                    failures.resume();
                }
            }
        };

        if let Some(cleanup) = self.complete_failure(&message) {
            let mut failure = self.failure.lock().expect(NEVER_POISONED);
            match &mut *failure {
                FailureState::Retained(retained) => retained.append(cleanup),
                FailureState::Unavailable => {
                    drop(failure);
                    cleanup.resume();
                }
                // First publication installs Retained or Unavailable before cleanup starts.
                FailureState::Vacant => unreachable!("failure cleanup follows publication"),
            }
        }
    }

    /// Publishes a failure whose submitting thread owns the payload through diagnostic reentry.
    ///
    /// Only first publication performs diagnostics. If a failure was already retained, it
    /// outranks the new submission panic but not incidental cleanup panics.
    pub(crate) fn fail_for_submission(
        &self,
        payload: Box<dyn Any + Send>,
        rejection_cleanup: Option<PanicFailures>,
    ) -> PanicFailures {
        let previous = {
            let mut failure = self.failure.lock().expect(NEVER_POISONED);
            replace(&mut *failure, FailureState::Unavailable)
        };

        let mut failures = PanicFailures::new(payload);
        let message = match previous {
            FailureState::Vacant => Some(extract_panic_message(failures.latest())),
            FailureState::Retained(retained) => {
                // Keep the displaced submission panic for disposal after draining as well.
                failures.append(retained);
                None
            }
            FailureState::Unavailable => None,
        };
        if let Some(cleanup) = rejection_cleanup {
            failures.append(cleanup);
        }
        if let Some(message) = message
            && let Some(cleanup) = self.complete_failure(&message)
        {
            failures.append(cleanup);
        }

        failures
    }

    /// Reports first publication and drains queues without disposing of caught payloads.
    fn complete_failure(&self, message: &str) -> Option<PanicFailures> {
        // Terminal state must be visible before diagnostics can re-enter the pool.
        // The failure-state mutex is no longer held while tracing invokes subscribers.
        self.stop_admission();
        let mut failures = catch_unwind(AssertUnwindSafe(|| {
            error!(
                target: POOL_LOG_TARGET,
                pool_id = self.pool_id,
                message,
                "worker pool infrastructure failed"
            );
        }))
        .err()
        .map(PanicFailures::new);
        self.drain_groups(|result| {
            if let Err(payload) = result {
                let message = extract_panic_message(&payload);
                let mut cleanup = PanicFailures::new(payload);
                cleanup.record_diagnostic(|| {
                    error!(message, "failure during worker pool cleanup");
                });
                PanicFailures::accumulate(&mut failures, cleanup);
            }
        });
        failures
    }

    /// Drains every registered queue outside the registry lock after terminal publication.
    pub(crate) fn drain_groups(&self, mut record_failure: impl FnMut(ThreadResult<()>)) {
        let groups = self
            .groups
            .lock()
            .expect(NEVER_POISONED)
            .iter()
            .map(|group| Arc::clone(&group.state))
            .collect::<Vec<_>>();
        for group in groups {
            drain_queue(&group.urgent_queue, &mut record_failure);
            drain_queue(&group.regular_queue, &mut record_failure);
        }
    }

    /// Transfers a runtime-retained failure exactly once to the current cleanup owner.
    ///
    /// An unavailable submission-owned payload remains unavailable so reentrant shutdown
    /// cannot consume it.
    pub(crate) fn take_failure(&self) -> Option<PanicFailures> {
        let mut failure = self.failure.lock().expect(NEVER_POISONED);
        match replace(&mut *failure, FailureState::Unavailable) {
            FailureState::Retained(failures) => Some(failures),
            FailureState::Vacant => {
                *failure = FailureState::Vacant;
                None
            }
            FailureState::Unavailable => None,
        }
    }
}

/// Keeps one group state alive for runtime-wide signaling and terminal cleanup.
///
/// Registration decouples worker lifecycle management from policy-owned `WorkerGroup` handles,
/// so shutdown can reach every created group even if a policy drops its own handle.
struct RegisteredGroup {
    state: Arc<WorkerGroupState>,
}

/// Tracks which boundary owns propagation of infrastructure and incidental failures.
///
/// Retained payloads stay in occurrence order so the most recent incidental failure becomes
/// primary when a propagation boundary consumes them. Keeping ownership explicit prevents a
/// worker, submission and later cleanup from consuming the same arbitrary payload concurrently.
enum FailureState {
    Vacant,
    Retained(PanicFailures),
    Unavailable,
}

/// Counts live participation and outstanding binding targets for shutdown barriers.
///
/// The runtime uses one synchronized lifecycle so policy binding, worker startup and
/// pool destruction agree on when every participating thread has left the pool.
#[derive(Default)]
struct WorkerLifecycle {
    active: usize,
    binding: bool,
    binding_workers: usize,
}

fn drain_queue(
    queue: &Mutex<VecDeque<ErasedTaskHandle>>,
    record_failure: &mut impl FnMut(ThreadResult<()>),
) {
    loop {
        let task = queue.lock().expect(NEVER_POISONED).pop_front();
        let Some(task) = task else {
            return;
        };

        record_failure(catch_unwind(AssertUnwindSafe(|| drop(task))));
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::any::Any;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use tracing::span::{Attributes, Id, Record};
    use tracing::subscriber::with_default;
    use tracing::{Event, Metadata, Subscriber};

    use super::*;

    /// Verifies failure state before a tracing callback attempts to re-enter runtime access.
    struct InspectingSubscriber {
        runtime: Arc<PoolRuntime>,
        state_was_ready: Arc<AtomicBool>,
    }

    impl Subscriber for InspectingSubscriber {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, _event: &Event<'_>) {
            // The callback runs synchronously in with_default; this flag communicates the
            // observation back to the same thread, not a cross-thread lifecycle barrier.
            self.state_was_ready.store(
                self.runtime.is_shutdown()
                    && self.runtime.is_poisoned()
                    && self.runtime.failure.try_lock().is_ok(),
                Ordering::Relaxed,
            );
            panic_any("failure subscriber panicked");
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }

    #[test]
    fn diagnostic_panic_replaces_retained_infrastructure_failure() {
        /// Identifies the original infrastructure payload before diagnostics run.
        struct OriginalFailure;

        let runtime = PoolRuntime::new(1, "test".to_string(), None);
        let state_was_ready = Arc::new(AtomicBool::new(false));
        let subscriber = InspectingSubscriber {
            runtime: Arc::clone(&runtime),
            state_was_ready: Arc::clone(&state_was_ready),
        };

        with_default(subscriber, || {
            let failure: Box<dyn Any + Send> = Box::new(OriginalFailure);
            runtime.fail(PanicFailures::new(failure));
        });

        assert!(state_was_ready.load(Ordering::Relaxed));
        let failure = runtime
            .take_failure()
            .expect("the diagnostic panic becomes the retained primary failure")
            .into_latest();
        assert_eq!(
            *failure
                .downcast::<&'static str>()
                .expect("subscriber panic produces the standard string payload"),
            "failure subscriber panicked"
        );
    }
}
