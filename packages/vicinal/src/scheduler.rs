//! Scheduler for spawning tasks on the worker pool.

use std::sync::Arc;

use tracing::trace;

use crate::metrics::CLOCK;
use crate::pool_inner::POOL_LOG_TARGET;
use crate::{
    ErasedTaskHandle, JoinHandle, NEVER_POISONED, PoolInner, VicinalTask, WorkerGroupState,
    init_task, wrap_task, wrap_task_and_forget,
};

/// A handle for spawning tasks on a [`Pool`][crate::Pool].
///
/// Each task uses the selected
/// [`WorkerPolicy`][crate::worker_policy::WorkerPolicy] to choose a group before
/// admission. The built-in processor policy uses the submitting processor, while the regional
/// and system policies select shared destinations across larger locality scopes.
/// Built-in shared-worker affinity includes the eager selection and processors that reach
/// policy selection for that destination. All submission methods use the same policy.
/// Tasks execute synchronously and must preserve worker affinity.
///
/// # Cloning
///
/// Schedulers are cheaply cloneable and can be shared across threads. All clones
/// submit work to the same pool.
///
/// # Lifetime
///
/// Scheduler clones do not keep worker threads running after their [`Pool`][crate::Pool]
/// is dropped. Submissions after clean shutdown are cancelled without execution. Joined tasks
/// retain their captures for recovery through
/// [`TaskError::into_body()`][crate::TaskError::into_body]. Fire-and-forget tasks release their
/// captures before returning.
///
/// # Worker failures
///
/// Infrastructure failures, including thread creation, initialization and affinity setup or
/// refresh, are logged and stop the entire pool. Queued joined tasks are cancelled recoverably;
/// queued fire-and-forget tasks release their captures. Executing tasks may finish.
/// The pool becomes permanently poisoned: every subsequent submission panics synchronously,
/// including through retained schedulers after the pool is dropped.
///
/// A failure detected during submission panics on that call. A worker-side failure may
/// occur after the triggering submission returns and is propagated when the pool is dropped.
/// Task panics retain their separate joined/fire-and-forget behavior and do not poison the pool.
///
/// # Example
///
/// ```rust
/// # use std::error::Error;
/// use vicinal::Pool;
/// # use futures::executor::block_on;
///
/// # block_on(async {
/// let pool = Pool::new();
/// let scheduler = pool.scheduler();
///
/// // Spawn from the main thread.
/// let handle1 = scheduler.spawn(|| "hello");
///
/// // Clone and use in another context.
/// let scheduler2 = scheduler.clone();
/// let handle2 = scheduler.spawn(move || scheduler2.spawn(|| "nested"));
///
/// assert_eq!(handle1.await?, "hello");
/// assert_eq!(handle2.await?.await?, "nested");
/// # Ok::<(), Box<dyn Error>>(())
/// # }).unwrap();
/// ```
#[derive(Clone, Debug)]
pub struct Scheduler {
    inner: Arc<PoolInner>,
}

impl Scheduler {
    /// Creates a new scheduler for the given pool.
    pub(crate) fn new(inner: Arc<PoolInner>) -> Self {
        Self { inner }
    }

    /// Spawns a task near the caller, according to the pool's worker policy.
    ///
    /// The task has normal priority and executes within the policy's permitted processor set.
    ///
    /// Spawned tasks may be executed in any order. Urgent tasks are scheduled
    /// preferentially, without preempting tasks that are already executing.
    ///
    /// # Returns
    ///
    /// A [`JoinHandle`] yielding the task's return value or a [`TaskError`][crate::TaskError].
    /// Task panics are returned as errors, as is cancellation before execution.
    ///
    /// # Panics
    ///
    /// This call panics on [worker failure](Self#worker-failures).
    pub fn spawn<R, F>(&self, task: F) -> JoinHandle<R>
    where
        R: Send + 'static,
        F: FnOnce() -> R + Send + 'static,
    {
        self.inner
            .submit(task, |task| self.spawn_internal(task, false))
    }

    /// Spawns an urgent task near the caller, according to the worker policy.
    ///
    /// Urgent tasks are preferred over regular tasks. Use this for time-sensitive work.
    ///
    /// Spawned tasks may be executed in any order. Urgent tasks are scheduled
    /// preferentially, without preempting tasks that are already executing.
    ///
    /// # Returns
    ///
    /// A [`JoinHandle`] yielding the task's return value or a [`TaskError`][crate::TaskError].
    /// Task panics are returned as errors, as is cancellation before execution.
    ///
    /// # Panics
    ///
    /// This call panics on [worker failure](Self#worker-failures).
    pub fn spawn_urgent<R, F>(&self, task: F) -> JoinHandle<R>
    where
        R: Send + 'static,
        F: FnOnce() -> R + Send + 'static,
    {
        self.inner
            .submit(task, |task| self.spawn_internal(task, true))
    }

    /// Spawns a fire-and-forget task according to the pool's worker policy.
    ///
    /// Unlike [`spawn`](Self::spawn), this method does not return a [`JoinHandle`].
    /// The task's result is discarded, and panics are logged rather than propagated.
    ///
    /// Use this when:
    /// - The task's return value is not needed
    /// - Panic propagation is not required
    /// - Working with trait objects that cannot support generic return types
    ///
    /// # Panics
    ///
    /// This call panics on [worker failure](Self#worker-failures).
    ///
    /// Task panics are logged but are not propagated to the caller.
    pub fn spawn_and_forget<F>(&self, task: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.inner
            .submit(task, |task| self.spawn_internal_and_forget(task, false));
    }

    /// Spawns an urgent fire-and-forget task according to the worker policy.
    ///
    /// This is the fire-and-forget equivalent of [`spawn_urgent`](Self::spawn_urgent).
    /// The task has urgent priority but does not return a [`JoinHandle`].
    ///
    /// Use this for time-sensitive operations where the result is not needed.
    ///
    /// # Panics
    ///
    /// This call panics on [worker failure](Self#worker-failures).
    ///
    /// Task panics are logged but are not propagated to the caller.
    pub fn spawn_urgent_and_forget<F>(&self, task: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.inner
            .submit(task, |task| self.spawn_internal_and_forget(task, true));
    }

    /// Prepares a joined task while retaining its captures across each fallible ownership transfer.
    fn spawn_internal<R, F>(&self, task: &mut Option<F>, urgent: bool) -> JoinHandle<R>
    where
        R: Send + 'static,
        F: FnOnce() -> R + Send + 'static,
    {
        if self.inner.runtime.is_shutdown() {
            trace!(
                target: POOL_LOG_TARGET,
                pool_id = self.inner.pool_id,
                "abandoned task during shutdown"
            );
            // Read the fallible clock before moving ownership out of the task option. If it
            // panics, submit's cleanup boundary can still contain and release the capture.
            let spawn_time = CLOCK.with_borrow_mut(fast_time::Clock::now);
            let (sender, receiver) = self.inner.runtime.result_channel_pool.rent();
            drop(wrap_task(
                task.take()
                    .expect("submit installs this task before any fallible shutdown operation"),
                sender,
                spawn_time,
            ));
            return JoinHandle::new(receiver);
        }

        let spawn_time = CLOCK.with_borrow_mut(fast_time::Clock::now);
        let group = self.inner.policy.select_group(&self.inner.groups);
        let state = self.inner.groups.resolve(group);
        let (sender, receiver) = state.result_channel_pool.rent();

        let wrapped = wrap_task(
            task.take()
                .expect("submission keeps the task in this Option until first wrapping"),
            sender,
            spawn_time,
        );

        // Retain both endpoints outside preparation's unwind: cancellation can move captures
        // into the receiver, whose destruction must also happen behind the cleanup boundary.
        self.inner.submit((Some(wrapped), receiver), |submission| {
            let (wrapped, _) = submission
                .as_mut()
                .expect("nested submit moves the wrapped task only after this borrow is released");
            let task = allocate_task(state, wrapped);
            self.inner.enqueue(state, task, urgent);

            let (_, receiver) = submission.take().expect(
                "successful admission is the sole branch that consumes the result receiver",
            );
            JoinHandle::new(receiver)
        })
    }

    /// Prepares a task without a result channel, retaining captures across allocation failure.
    fn spawn_internal_and_forget<F>(&self, task: &mut Option<F>, urgent: bool)
    where
        F: FnOnce() + Send + 'static,
    {
        if self.inner.runtime.is_shutdown() {
            trace!(
                target: POOL_LOG_TARGET,
                pool_id = self.inner.pool_id,
                "abandoned task during shutdown"
            );
            drop(
                task.take()
                    .expect("shutdown wrapping is the first operation that consumes this task"),
            );
            return;
        }

        let spawn_time = CLOCK.with_borrow_mut(fast_time::Clock::now);
        let group = self.inner.policy.select_group(&self.inner.groups);
        let state = self.inner.groups.resolve(group);

        let wrapped = wrap_task_and_forget(
            task.take()
                .expect("fire-and-forget wrapping is the first operation that consumes this task"),
            spawn_time,
        );

        self.inner.submit(wrapped, |wrapped| {
            let task = allocate_task(state, wrapped);
            self.inner.enqueue(state, task, urgent);
        });
    }
}

/// Allocates and initializes a task without invoking task code under the pool lock.
fn allocate_task<T: VicinalTask>(
    state: &WorkerGroupState,
    task: &mut Option<T>,
) -> ErasedTaskHandle {
    let slot = {
        let pool = state.task_pool.lock().expect(NEVER_POISONED);
        pool.try_alloc_uninit_box()
    };

    let slot = match slot {
        Ok(slot) => slot,
        Err(error) => panic!("task pool allocation failed: {error}"),
    };

    init_task(
        slot,
        task.take()
            .expect("initialization consumes the wrapped task only after allocation succeeds"),
    )
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::mem::forget;
    use std::panic;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use events_once::EventPool;
    use futures::executor::block_on;
    use many_cpus::SystemHardware;
    use many_cpus::fake::HardwareBuilder;
    use new_zealand::nz;
    use static_assertions::assert_impl_all;
    use testing::{assert_panics, assert_panics_with, with_watchdog};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Metadata, Subscriber};

    use crate::metrics::CLOCK;
    use crate::pool_inner::{OnDrop, POOL_LOG_TARGET, PanickingPayload, TestPolicy};
    use crate::{Pool, Scheduler};

    assert_impl_all!(Scheduler: UnwindSafe, RefUnwindSafe);

    /// Drops a captured pool when it receives the infrastructure-failure diagnostic.
    struct PoolDroppingSubscriber {
        pool: Mutex<Option<Pool>>,
        dropped: Arc<AtomicUsize>,
    }

    impl Subscriber for PoolDroppingSubscriber {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            if event.metadata().target() == POOL_LOG_TARGET
                && let Some(pool) = self.pool.lock().unwrap().take()
            {
                drop(pool);
                // The subscriber call is synchronous; this counter only reports that its
                // same-thread reentrant action completed.
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }

    fn fake_pool() -> Pool {
        Pool::builder()
            .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
                nz!(1),
                nz!(1),
            )))
            .worker_policy(TestPolicy::new(1))
            .build()
            .unwrap()
    }

    fn assert_incidental_panic(f: impl FnOnce()) {
        let payload = panic::catch_unwind(panic::AssertUnwindSafe(f)).unwrap_err();
        assert!(payload.is::<PanickingPayload>());
        forget(payload);
    }

    /// Rejected capture cleanup must remain contained even through a retained poisoned scheduler.
    fn assert_poisoned_capture_released(submit: impl FnOnce(&Scheduler, OnDrop)) {
        let pool = fake_pool();
        let scheduler = pool.scheduler();
        pool.inner.stop_admission();
        drop(pool);

        let dropped = Arc::new(AtomicUsize::new(0));
        let capture = OnDrop(Some(Box::new({
            let dropped = Arc::clone(&dropped);
            move || {
                dropped.fetch_add(1, Ordering::Relaxed);
                panic::panic_any(PanickingPayload);
            }
        })));

        assert_incidental_panic(|| submit(&scheduler, capture));

        assert_eq!(dropped.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn poisoned_spawn_contains_capture_destruction() {
        assert_poisoned_capture_released(|scheduler, capture| {
            _ = scheduler.spawn(move || drop(capture));
        });
    }

    #[test]
    fn poisoned_urgent_spawn_contains_capture_destruction() {
        assert_poisoned_capture_released(|scheduler, capture| {
            _ = scheduler.spawn_urgent(move || drop(capture));
        });
    }

    #[test]
    fn poisoned_forget_contains_capture_destruction() {
        assert_poisoned_capture_released(|scheduler, capture| {
            scheduler.spawn_and_forget(move || drop(capture));
        });
    }

    #[test]
    fn poisoned_urgent_forget_contains_capture_destruction() {
        assert_poisoned_capture_released(|scheduler, capture| {
            scheduler.spawn_urgent_and_forget(move || drop(capture));
        });
    }

    /// An existing clock borrow forces failure before topology/channel preparation
    /// consumes the task.
    #[test]
    fn preparation_failure_contains_unwrapped_capture() {
        let pool = fake_pool();
        let scheduler = pool.scheduler();
        let capture = OnDrop(Some(Box::new(|| panic::panic_any(PanickingPayload))));

        CLOCK.with_borrow_mut(|_| {
            assert_incidental_panic(|| {
                _ = scheduler.spawn(move || drop(capture));
            });
        });

        assert!(pool.inner.runtime.is_poisoned());
        assert!(pool.inner.runtime.is_shutdown());
        pool.inner.runtime.wait_for_workers();
        assert_eq!(pool.inner.runtime.active_workers(), 0);
    }

    #[test]
    fn preparation_cleanup_drains_queued_tasks_before_panicking() {
        let pool = Pool::builder()
            .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
                nz!(1),
                nz!(1),
            )))
            .worker_policy(TestPolicy::new(0))
            .build()
            .unwrap();
        let scheduler = pool.scheduler();
        let queued = scheduler.spawn(|| 42);
        let capture = OnDrop(Some(Box::new(|| panic::panic_any(PanickingPayload))));

        CLOCK.with_borrow_mut(|_| {
            assert_incidental_panic(|| {
                _ = scheduler.spawn(move || drop(capture));
            });
        });

        assert!(block_on(queued).unwrap_err().is_cancel());
        assert!(pool.inner.runtime.is_poisoned());
        assert!(pool.inner.runtime.is_shutdown());
    }

    /// Shutdown-time clock failure must still retain the unconsumed task for cleanup.
    #[test]
    fn shutdown_preparation_failure_contains_unwrapped_capture() {
        let pool = fake_pool();
        let scheduler = pool.scheduler();
        drop(pool);
        let capture = OnDrop(Some(Box::new(|| panic::panic_any(PanickingPayload))));

        CLOCK.with_borrow_mut(|_| {
            assert_incidental_panic(|| {
                _ = scheduler.spawn(move || drop(capture));
            });
        });

        assert!(scheduler.inner.runtime.is_poisoned());
        assert!(scheduler.inner.runtime.is_shutdown());
        scheduler.inner.runtime.wait_for_workers();
        assert_eq!(scheduler.inner.runtime.active_workers(), 0);
    }

    /// A poisoned allocation lock forces failure after wrapping but before task storage exists.
    #[test]
    fn allocation_failure_contains_wrapped_capture() {
        with_watchdog(|| {
            for joined in [false, true] {
                let pool = fake_pool();
                let scheduler = pool.scheduler();
                let group = pool.inner.policy.select_group(&pool.inner.groups);
                let state = pool.inner.groups.resolve(group);

                assert_panics(|| {
                    let _guard = state.task_pool.lock().unwrap();
                    panic!("injected allocator lock failure");
                });

                let capture = OnDrop(Some(Box::new(|| panic::panic_any(PanickingPayload))));
                assert_incidental_panic(|| {
                    if joined {
                        _ = scheduler.spawn(move || drop(capture));
                    } else {
                        scheduler.spawn_and_forget(move || drop(capture));
                    }
                });

                assert!(pool.inner.runtime.is_poisoned());
                assert!(pool.inner.runtime.is_shutdown());
                pool.inner.runtime.wait_for_workers();
                assert_eq!(pool.inner.runtime.active_workers(), 0);
            }
        });
    }

    #[test]
    fn failure_diagnostic_can_drop_pool_on_submitting_thread() {
        with_watchdog(|| {
            let pool = fake_pool();
            let scheduler = pool.scheduler();
            let group = pool.inner.policy.select_group(&pool.inner.groups);
            let state = pool.inner.groups.resolve(group);

            assert_panics(|| {
                let _guard = state.task_pool.lock().unwrap();
                panic!("injected allocator lock failure");
            });

            let dropped = Arc::new(AtomicUsize::new(0));
            let subscriber = PoolDroppingSubscriber {
                pool: Mutex::new(Some(pool)),
                dropped: Arc::clone(&dropped),
            };

            tracing::subscriber::with_default(subscriber, || {
                assert_panics_with(
                    || {
                        _ = scheduler.spawn(|| 42);
                    },
                    |message| assert!(message.contains("we never panic while holding this lock")),
                );
            });

            assert_eq!(dropped.load(Ordering::Relaxed), 1);
            assert!(scheduler.inner.runtime.is_poisoned());
            scheduler.inner.runtime.wait_for_workers();
            assert_eq!(scheduler.inner.runtime.active_workers(), 0);
        });
    }

    #[test]
    fn spawn_returns_join_handle() {
        let pool = fake_pool();
        let scheduler = pool.scheduler();

        let handle = scheduler.spawn(|| 42);
        let result = block_on(handle).unwrap();

        assert_eq!(result, 42);
    }

    #[test]
    fn spawn_urgent_returns_join_handle() {
        let pool = fake_pool();
        let scheduler = pool.scheduler();

        let handle = scheduler.spawn_urgent(|| "urgent result");
        let result = block_on(handle).unwrap();

        assert_eq!(result, "urgent result");
    }

    #[test]
    fn scheduler_is_cloneable() {
        let pool = fake_pool();
        let scheduler = pool.scheduler();
        let scheduler2 = scheduler.clone();

        let handle1 = scheduler.spawn(|| 1);
        let handle2 = scheduler2.spawn(|| 2);

        assert_eq!(block_on(handle1).unwrap(), 1);
        assert_eq!(block_on(handle2).unwrap(), 2);
    }

    #[test]
    fn spawn_captures_panic() {
        let pool = fake_pool();
        let scheduler = pool.scheduler();

        let handle = scheduler.spawn(|| {
            panic!("test panic");
        });

        assert!(block_on(handle).unwrap_err().is_panic());
    }

    #[test]
    fn spawn_accepts_not_unpin_task() {
        let pool = fake_pool();
        let scheduler = pool.scheduler();

        // Create a !Unpin task by capturing a !Unpin future.
        let not_unpin_future = async {};
        let handle = scheduler.spawn(move || {
            let _unused = not_unpin_future;
            42
        });

        let result = block_on(handle).unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn spawn_and_forget_completes() {
        let pool = fake_pool();
        let scheduler = pool.scheduler();
        let event_pool = EventPool::<()>::new();

        let (tx, rx) = event_pool.rent();
        scheduler.spawn_and_forget(move || {
            tx.send(());
        });

        block_on(rx).unwrap();
    }

    #[test]
    fn spawn_urgent_and_forget_completes() {
        let pool = fake_pool();
        let scheduler = pool.scheduler();
        let event_pool = EventPool::<()>::new();

        let (tx, rx) = event_pool.rent();
        scheduler.spawn_urgent_and_forget(move || {
            tx.send(());
        });

        block_on(rx).unwrap();
    }
}
