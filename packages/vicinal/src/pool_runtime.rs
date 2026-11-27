//! Pool-wide terminal state shared by every policy-created worker group.

use std::any::Any;
use std::mem::replace;
use std::ops::ControlFlow;
#[cfg(test)]
use std::panic::panic_any;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
#[cfg(test)]
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::Result as ThreadResult;

use events_once::EventLake;
use tracing::error;

use crate::pool_inner::POOL_LOG_TARGET;
use crate::{
    ErasedTaskHandle, NEVER_POISONED, ThreadInit, WorkerGroupState, extract_panic_message,
    record_diagnostic_panic, resume_latest_failure,
};

/// Coordinates terminal state, group registration, failure ownership and worker participation.
///
/// It is shared by the pool, schedulers and every policy-created worker. Ref:
/// docs/implementation.md, "Ownership and coordination" and "Affinity and failure coordination".
pub(crate) struct PoolRuntime {
    pool_id: u64,
    pool_name: Arc<str>,
    registry_owner: OnceLock<u64>,
    thread_init: Option<ThreadInit>,
    shutdown: AtomicBool,
    poisoned: AtomicBool,
    groups: Mutex<Vec<RegisteredGroup>>,
    workers: Mutex<WorkerLifecycle>,
    workers_changed: Condvar,
    #[cfg(test)]
    pub(crate) before_worker_wait:
        OnceLock<Box<dyn Fn() + Send + Sync + UnwindSafe + RefUnwindSafe>>,
    failure: Mutex<FailureState>,
    pub(crate) result_channel_pool: EventLake,
}

/// Keeps one group state alive for runtime-wide signaling and terminal cleanup.
///
/// Registration decouples worker lifecycle management from policy-owned `WorkerGroup` handles,
/// so shutdown can reach every created group even if a policy drops its own handle.
struct RegisteredGroup {
    state: Arc<WorkerGroupState>,
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
            registry_owner: OnceLock::new(),
            thread_init,
            shutdown: AtomicBool::new(false),
            poisoned: AtomicBool::new(false),
            groups: Mutex::new(Vec::new()),
            workers: Mutex::new(WorkerLifecycle::default()),
            workers_changed: Condvar::new(),
            #[cfg(test)]
            before_worker_wait: OnceLock::new(),
            failure: Mutex::new(FailureState::Vacant),
            result_channel_pool: EventLake::new(),
        })
    }

    pub(crate) fn set_registry_owner(&self, owner: u64) {
        self.registry_owner
            .set(owner)
            .expect("PoolRuntime::new creates one registry and calls set_registry_owner once");
    }

    pub(crate) fn registry_owner(&self) -> u64 {
        *self
            .registry_owner
            .get()
            .expect("WorkerGroups::new installs the owner before add() can create a group")
    }

    pub(crate) fn pool_id(&self) -> u64 {
        self.pool_id
    }

    pub(crate) fn pool_name(&self) -> &str {
        &self.pool_name
    }

    #[cfg(test)]
    pub(crate) fn pool_name_snapshot(&self) -> Arc<str> {
        Arc::clone(&self.pool_name)
    }

    pub(crate) fn thread_init(&self) -> Option<&ThreadInit> {
        self.thread_init.as_ref()
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

    /// Cancels one worker-initialization target without running that worker.
    // Omitting this decrement and notification can leave binding waiting forever.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn cancel_binding_worker(&self) {
        let mut workers = self.workers.lock().expect(NEVER_POISONED);
        workers.binding_workers = workers
            .binding_workers
            .checked_sub(1)
            .expect("BindingReservation owns one outstanding reservation until cancellation");
        self.workers_changed.notify_all();
    }

    /// Completes one tracked binding worker after its initializer returns.
    // Omitting this decrement and notification can leave binding waiting forever.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn initialize_binding_worker(&self) {
        let mut workers = self.workers.lock().expect(NEVER_POISONED);
        workers.binding_workers = workers
            .binding_workers
            .checked_sub(1)
            .expect("each initialized binding worker owns exactly one reservation");
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
    pub(crate) fn fail(&self, payload: Box<dyn Any + Send>) {
        let mut payload = Some(payload);
        let message = {
            let mut failure = self.failure.lock().expect(NEVER_POISONED);
            if matches!(*failure, FailureState::Vacant) {
                let message =
                    extract_panic_message(payload.as_ref().expect(
                        "Some is assigned for the vacant first-failure branch before take",
                    ));
                *failure = FailureState::Retained(vec![
                    payload
                        .take()
                        .expect("the payload remains Some until this first-failure move"),
                ]);
                Some(message)
            } else if let FailureState::Retained(failures) = &mut *failure {
                failures.push(
                    payload
                        .take()
                        .expect("the payload remains Some until this later-failure move"),
                );
                drop(failure);
                return;
            } else {
                None
            }
        };

        let Some(message) = message else {
            // The pool is already terminal; this later panic takes precedence on this thread.
            resume_unwind(
                payload.expect("the non-vacant branch neither takes nor replaces this payload"),
            );
        };

        self.complete_failure(&message);
    }

    /// Publishes a failure whose submitting thread owns the payload through diagnostic reentry.
    ///
    /// A vacant failure belongs to this call, which publishes it and returns the same payload
    /// after terminal cleanup. If another failure is retained, this incidental failure takes
    /// precedence and propagates after releasing the retained payloads. An unavailable failure
    /// remains submission-owned, so this call returns its payload without publishing or draining
    /// again. Only first publication completes failure cleanup.
    pub(crate) fn fail_for_submission(
        &self,
        payload: Box<dyn Any + Send>,
    ) -> Vec<Box<dyn Any + Send>> {
        let (message, retained) = {
            let mut failure = self.failure.lock().expect(NEVER_POISONED);
            match replace(&mut *failure, FailureState::Unavailable) {
                FailureState::Vacant => (Some(extract_panic_message(&payload)), None),
                FailureState::Retained(failures) => (None, Some(failures)),
                FailureState::Unavailable => (None, None),
            }
        };

        if let Some(mut retained) = retained {
            if let Err(cleanup) = catch_unwind(AssertUnwindSafe(|| drop(payload))) {
                retained.push(cleanup);
            }
            return retained;
        }

        if let Some(message) = message {
            self.complete_failure(&message);
        }

        vec![payload]
    }

    fn complete_failure(&self, message: &str) {
        // Terminal state must be visible before diagnostics can re-enter the pool.
        // The failure-state mutex is no longer held while tracing invokes subscribers.
        self.stop_admission();
        let mut failures = Vec::new();
        record_diagnostic_panic(&mut failures, || {
            error!(
                target: POOL_LOG_TARGET,
                pool_id = self.pool_id,
                message,
                "worker pool infrastructure failed"
            );
        });
        self.drain_groups(|result| {
            if let Err(payload) = result {
                let message = extract_panic_message(&payload);
                failures.push(payload);
                record_diagnostic_panic(&mut failures, || {
                    error!(message, "failure during worker pool cleanup");
                });
            }
        });
        if !failures.is_empty() {
            let mut pending = Some(failures);
            let submission_owned = {
                let mut failure = self.failure.lock().expect(NEVER_POISONED);
                match &mut *failure {
                    FailureState::Retained(retained) => {
                        retained
                            .extend(pending.take().expect("pending failures are consumed once"));
                        false
                    }
                    FailureState::Vacant => {
                        *failure = FailureState::Retained(
                            pending.take().expect("pending failures are consumed once"),
                        );
                        false
                    }
                    FailureState::Unavailable => true,
                }
            };
            if submission_owned {
                resume_latest_failure(
                    pending.expect("submission-owned failures remain available after lookup"),
                );
            }
        }
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
    pub(crate) fn take_failure(&self) -> Option<Box<dyn Any + Send>> {
        let mut failure = self.failure.lock().expect(NEVER_POISONED);
        let previous = replace(&mut *failure, FailureState::Unavailable);
        match previous {
            FailureState::Retained(mut failures) => {
                drop(failure);
                let latest = failures
                    .pop()
                    .expect("retained failure state always contains one payload");
                for earlier in failures {
                    drop(earlier);
                }
                Some(latest)
            }
            FailureState::Vacant => {
                *failure = FailureState::Vacant;
                None
            }
            FailureState::Unavailable => None,
        }
    }
}

/// Tracks which boundary owns propagation of infrastructure and incidental failures.
///
/// Retained payloads stay in occurrence order so the most recent incidental failure becomes
/// primary when a propagation boundary consumes them. Keeping ownership explicit prevents a
/// worker, submission and later cleanup from consuming the same arbitrary payload concurrently.
enum FailureState {
    Vacant,
    Retained(Vec<Box<dyn Any + Send>>),
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
    queue: &Mutex<std::collections::VecDeque<ErasedTaskHandle>>,
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
                Ordering::Release,
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

        tracing::subscriber::with_default(subscriber, || {
            let failure: Box<dyn Any + Send> = Box::new(OriginalFailure);
            runtime.fail(failure);
        });

        assert!(state_was_ready.load(Ordering::Acquire));
        let failure = runtime
            .take_failure()
            .expect("the diagnostic panic becomes the retained primary failure");
        assert_eq!(
            *failure
                .downcast::<&'static str>()
                .expect("subscriber panic produces the standard string payload"),
            "failure subscriber panicked"
        );
    }
}
