//! Pool-wide terminal state shared by every policy-created worker group.

use std::any::Any;
use std::cell::RefCell;
use std::mem::replace;
use std::ops::ControlFlow;
#[cfg(test)]
use std::panic::panic_any;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
#[cfg(test)]
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use events_once::EventLake;
use tracing::error;

use crate::pool_inner::POOL_LOG_TARGET;
use crate::{
    ErasedTaskHandle, NEVER_POISONED, ThreadInit, WorkerGroupState, discard_panic,
    extract_panic_message, report_diagnostic,
};

thread_local! {
    /// Counts every active worker scope for each runtime on this thread.
    static CURRENT_WORKER_RUNTIMES: RefCell<Vec<(NonNull<PoolRuntime>, usize)>> =
        const { RefCell::new(Vec::new()) };
}

/// Restores thread-local worker participation after one `Worker::run()` call.
pub(crate) struct WorkerScope {
    runtime: NonNull<PoolRuntime>,
}

impl Drop for WorkerScope {
    fn drop(&mut self) {
        CURRENT_WORKER_RUNTIMES.with_borrow_mut(|workers| {
            let (index, (_, count)) = workers
                .iter_mut()
                .enumerate()
                .find(|(_, (runtime, _))| *runtime == self.runtime)
                .expect("every worker scope has a matching runtime entry");
            if *count == 1 {
                workers.remove(index);
            } else {
                *count = count
                    .checked_sub(1)
                    .expect("an active scope contributes one worker");
            }
        });
    }
}

/// Coordinates terminal state, group registration, failure ownership and worker participation.
///
/// It is shared by the pool, schedulers and every policy-created worker. Ref:
/// docs/implementation.md, "Ownership and coordination" and "Affinity and failure coordination".
pub(crate) struct PoolRuntime {
    pool_id: u64,
    pool_name: String,
    registry_owner: OnceLock<u64>,
    thread_init: Option<ThreadInit>,
    shutdown: AtomicBool,
    poisoned: AtomicBool,
    groups: Mutex<Vec<Arc<WorkerGroupState>>>,
    workers: Mutex<WorkerLifecycle>,
    workers_changed: Condvar,
    #[cfg(test)]
    pub(crate) before_worker_wait:
        OnceLock<Box<dyn Fn() + Send + Sync + UnwindSafe + RefUnwindSafe>>,
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
            pool_name,
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
            .expect("one registry belongs to each pool runtime");
    }

    pub(crate) fn registry_owner(&self) -> u64 {
        *self
            .registry_owner
            .get()
            .expect("the pool registry is installed before groups are created")
    }

    pub(crate) fn pool_id(&self) -> u64 {
        self.pool_id
    }

    pub(crate) fn pool_name(&self) -> &str {
        &self.pool_name
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

    pub(crate) fn register_group(&self, group: &Arc<WorkerGroupState>) {
        let mut groups = self.groups.lock().expect(NEVER_POISONED);
        if self.is_poisoned() {
            drop(groups);
            panic!("worker groups cannot be added after pool failure");
        }

        groups.push(Arc::clone(group));
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

        workers.active = workers
            .active
            .checked_add(1)
            .expect("worker count cannot exceed addressable memory");
        ControlFlow::Continue(())
    }

    /// Marks the current thread as a participating worker of this pool.
    pub(crate) fn enter_worker(&self) -> WorkerScope {
        let runtime = NonNull::from(self);
        CURRENT_WORKER_RUNTIMES.with_borrow_mut(|workers| {
            if let Some((_, count)) = workers
                .iter_mut()
                .find(|(registered, _)| *registered == runtime)
            {
                *count = count
                    .checked_add(1)
                    .expect("worker nesting cannot exceed addressable memory");
            } else {
                workers.push((runtime, 1));
            }
        });
        WorkerScope { runtime }
    }

    fn current_worker_count(&self) -> usize {
        let runtime = NonNull::from(self);
        CURRENT_WORKER_RUNTIMES.with_borrow(|workers| {
            workers
                .iter()
                .find_map(|(registered, count)| (*registered == runtime).then_some(*count))
                .unwrap_or(0)
        })
    }

    /// Enables builder-owned tracking for workers reserved by policy preparation.
    pub(crate) fn begin_worker_preparation(&self) {
        let mut workers = self.workers.lock().expect(NEVER_POISONED);
        assert!(
            !workers.preparing,
            "worker preparation has not begun more than once"
        );
        workers.preparing = true;
    }

    /// Returns whether a worker belongs to the current policy-preparation phase.
    pub(crate) fn reserve_preparation_worker(&self) -> bool {
        let mut workers = self.workers.lock().expect(NEVER_POISONED);
        if !workers.preparing {
            return false;
        }

        workers.preparing_workers = workers
            .preparing_workers
            .checked_add(1)
            .expect("worker count cannot exceed addressable memory");
        true
    }

    /// Cancels one worker-initialization target without running that worker.
    pub(crate) fn cancel_preparation_worker(&self) {
        let mut workers = self.workers.lock().expect(NEVER_POISONED);
        workers.preparing_workers = workers
            .preparing_workers
            .checked_sub(1)
            .expect("only reserved preparation workers can be cancelled");
        self.workers_changed.notify_all();
    }

    /// Completes one tracked preparation worker after its initializer returns.
    pub(crate) fn initialize_preparation_worker(&self) {
        let mut workers = self.workers.lock().expect(NEVER_POISONED);
        workers.preparing_workers = workers
            .preparing_workers
            .checked_sub(1)
            .expect("only reserved preparation workers can initialize");
        self.workers_changed.notify_all();
    }

    /// Stops assigning newly reserved workers to the completed preparation phase.
    pub(crate) fn end_worker_preparation(&self) {
        self.workers.lock().expect(NEVER_POISONED).preparing = false;
    }

    /// Waits until policy-preparation workers initialize or the pool fails.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn wait_for_preparation_workers(&self) {
        let mut workers = self.workers.lock().expect(NEVER_POISONED);
        while workers.preparing_workers != 0 && !self.is_poisoned() {
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
            .expect("only registered workers can finish");
        self.workers_changed.notify_all();
    }

    #[cfg_attr(test, mutants::skip)]
    /// Publishes pool shutdown and wakes every registered group to leave its worker loop.
    pub(crate) fn stop_workers(&self) {
        // Release publishes the pool-wide terminal transition before the per-group wakeups.
        let groups = self.groups.lock().expect(NEVER_POISONED);
        self.shutdown.store(true, Ordering::Release);

        for group in groups.iter() {
            group.signal_shutdown();
            group.notify_initialization_failure();
            #[cfg(test)]
            group.mark_startup_failed_if_started(self.is_poisoned());
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
        // A task may own and drop the last Pool handle. The current worker cannot leave
        // until that task returns, so only other participating workers may be awaited here.
        let current_workers = self.current_worker_count();
        while workers.active > current_workers {
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

    /// Retains the first worker/runtime failure and contains later cleanup failures.
    ///
    /// Ref: docs/implementation.md, "Affinity and failure coordination".
    pub(crate) fn fail(&self, payload: Box<dyn Any + Send>) {
        let mut payload = Some(payload);
        let message = {
            let mut failure = self.failure.lock().expect(NEVER_POISONED);
            if matches!(*failure, FailureState::Vacant) {
                let message = extract_panic_message(
                    payload
                        .as_ref()
                        .expect("the first failure retains its original payload"),
                );
                *failure = FailureState::Retained(
                    payload
                        .take()
                        .expect("the first failure moves its retained payload"),
                );
                Some(message)
            } else {
                None
            }
        };

        let Some(message) = message else {
            // Keep the first infrastructure failure so later cleanup panics cannot
            // replace the root cause.
            discard_panic(
                payload.expect("an already-recorded failure leaves this payload for disposal"),
            );
            return;
        };

        self.complete_failure(&message);
    }

    /// Publishes a failure whose submitting thread owns the payload through diagnostic reentry.
    pub(crate) fn fail_for_submission(&self, payload: Box<dyn Any + Send>) -> Box<dyn Any + Send> {
        let (message, retained) = {
            let mut failure = self.failure.lock().expect(NEVER_POISONED);
            match replace(&mut *failure, FailureState::Unavailable) {
                FailureState::Vacant => (Some(extract_panic_message(&payload)), None),
                FailureState::Retained(payload) => (None, Some(payload)),
                FailureState::Unavailable => (None, None),
            }
        };

        if let Some(retained) = retained {
            discard_panic(payload);
            return retained;
        }

        if let Some(message) = message {
            self.complete_failure(&message);
        }

        payload
    }

    fn complete_failure(&self, message: &str) {
        // Terminal state must be visible before diagnostics can re-enter the pool.
        // The failure-state mutex is no longer held while tracing invokes subscribers.
        self.stop_admission();
        report_diagnostic(|| {
            error!(
                target: POOL_LOG_TARGET,
                pool_id = self.pool_id,
                message,
                "worker pool infrastructure failed"
            );
        });
        self.drain_groups();
    }

    /// Drains every registered queue outside the registry lock after failure publication.
    fn drain_groups(&self) {
        let groups = self.groups.lock().expect(NEVER_POISONED).clone();
        for group in groups {
            drain_queue(&group.urgent_queue);
            drain_queue(&group.regular_queue);
        }
    }

    pub(crate) fn resume_failure(&self) -> ! {
        let payload = self
            .take_failure()
            .expect("poisoned pool retains its original infrastructure failure");
        resume_unwind(payload)
    }

    /// Transfers a runtime-retained failure exactly once to the current cleanup owner.
    ///
    /// An unavailable submission-owned payload remains unavailable so reentrant shutdown
    /// cannot consume it.
    pub(crate) fn take_failure(&self) -> Option<Box<dyn Any + Send>> {
        let mut failure = self.failure.lock().expect(NEVER_POISONED);
        let previous = replace(&mut *failure, FailureState::Unavailable);
        match previous {
            FailureState::Retained(payload) => Some(payload),
            FailureState::Vacant => {
                *failure = FailureState::Vacant;
                None
            }
            FailureState::Unavailable => None,
        }
    }
}

/// Tracks whether infrastructure-failure ownership remains in runtime or an active caller.
enum FailureState {
    Vacant,
    Retained(Box<dyn Any + Send>),
    Unavailable,
}

#[derive(Default)]
struct WorkerLifecycle {
    active: usize,
    preparing: bool,
    preparing_workers: usize,
}

fn drain_queue(queue: &Mutex<std::collections::VecDeque<ErasedTaskHandle>>) {
    loop {
        let task = queue.lock().expect(NEVER_POISONED).pop_front();
        let Some(task) = task else {
            return;
        };

        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| drop(task))) {
            report_diagnostic(|| {
                error!(
                    message = extract_panic_message(&payload),
                    "failure during worker pool cleanup"
                );
            });
            discard_panic(payload);
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::any::Any;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use testing::with_watchdog;
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
    #[cfg_attr(
        miri,
        ignore = "subscriber panic payload retention intentionally leaks arbitrary callback state"
    )]
    fn failure_state_precedes_panicking_diagnostics() {
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
            .expect("diagnostic panic does not replace the infrastructure failure");
        assert!(failure.is::<OriginalFailure>());
    }

    #[test]
    fn nested_workers_do_not_wait_for_their_own_runtime() {
        with_watchdog(|| {
            let first = PoolRuntime::new(1, "first".to_string(), None);
            assert!(first.begin_worker().is_continue());
            let first_scope = first.enter_worker();

            assert!(first.begin_worker().is_continue());
            let nested_first_scope = first.enter_worker();
            first.stop_workers();
            first.wait_for_workers();

            drop(nested_first_scope);
            first.end_worker();
            drop(first_scope);
            first.end_worker();
        });
    }

    #[test]
    fn nested_other_runtime_preserves_outer_shutdown_scope() {
        with_watchdog(|| {
            let outer = PoolRuntime::new(1, "outer".to_string(), None);
            assert!(outer.begin_worker().is_continue());
            let outer_scope = outer.enter_worker();

            let inner = PoolRuntime::new(2, "inner".to_string(), None);
            assert!(inner.begin_worker().is_continue());
            let inner_scope = inner.enter_worker();

            outer.stop_workers();
            outer.wait_for_workers();

            drop(inner_scope);
            inner.end_worker();
            drop(outer_scope);
            outer.end_worker();
        });
    }
}
