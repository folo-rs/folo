//! Scheduler for spawning tasks on the worker pool.

use std::sync::Arc;

use crate::metrics::CLOCK;
use crate::{
    ErasedTaskHandle, JoinHandle, NEVER_POISONED, PoolInner, VicinalTask, WorkerGroup, init_task,
    wrap_task, wrap_task_and_forget,
};

/// A handle for spawning tasks on a [`Pool`][crate::Pool].
///
/// Every submission samples its destination once according to the pool's
/// [`WorkerPolicy`][crate::WorkerPolicy]: the current logical processor by default, or the current
/// memory region for a regional policy. All submission methods use the same policy.
/// Tasks execute synchronously and must preserve worker affinity.
///
/// # Cloning
///
/// Schedulers are cheaply cloneable and can be shared across threads. All clones
/// reference the same underlying pool.
///
/// # Example
///
/// ```rust
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
/// assert_eq!(handle1.await, "hello");
/// assert_eq!(handle2.await.await, "nested");
/// # });
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
    /// The task will be added to the regular (normal-priority) queue and executed
    /// by a worker assigned to the sampled processor or memory region.
    ///
    /// Spawned tasks may be executed in any order. Urgent tasks are scheduled
    /// preferentially but there is no hard guarantee that they will preempt all
    /// regular tasks.
    ///
    /// # Returns
    ///
    /// A [`JoinHandle`] that can be awaited to get the task's return value.
    ///
    /// # Panics
    ///
    /// If the task panics, the panic will be captured and re-thrown when the
    /// [`JoinHandle`] is awaited.
    pub fn spawn<R, F>(&self, task: F) -> JoinHandle<R>
    where
        R: Send + 'static,
        F: FnOnce() -> R + Send + 'static,
    {
        self.spawn_internal(task, false)
    }

    /// Spawns an urgent task near the caller, according to the worker policy.
    ///
    /// Urgent tasks are executed before regular tasks. Use this for time-sensitive
    /// operations that should not wait behind a queue of regular work.
    ///
    /// Spawned tasks may be executed in any order. Urgent tasks are scheduled
    /// preferentially but there is no hard guarantee that they will preempt all
    /// regular tasks.
    ///
    /// # Returns
    ///
    /// A [`JoinHandle`] that can be awaited to get the task's return value.
    ///
    /// # Panics
    ///
    /// If the task panics, the panic will be captured and re-thrown when the
    /// [`JoinHandle`] is awaited.
    pub fn spawn_urgent<R, F>(&self, task: F) -> JoinHandle<R>
    where
        R: Send + 'static,
        F: FnOnce() -> R + Send + 'static,
    {
        self.spawn_internal(task, true)
    }

    /// Spawns a fire-and-forget task according to the pool's worker policy.
    ///
    /// Unlike [`spawn`](Self::spawn), this method does not return a [`JoinHandle`].
    /// The task's result is discarded, and panics are logged rather than propagated.
    /// This reduces overhead by avoiding result channel allocation.
    ///
    /// Use this when:
    /// - The task's return value is not needed
    /// - Panic propagation is not required
    /// - Working with trait objects that cannot support generic return types
    ///
    /// # Panics
    ///
    /// If the task panics, the panic will be caught and logged via `tracing::error!`
    /// but will not be propagated to the caller.
    pub fn spawn_and_forget<F>(&self, task: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.spawn_internal_and_forget(task, false);
    }

    /// Spawns an urgent fire-and-forget task according to the worker policy.
    ///
    /// This is the fire-and-forget equivalent of [`spawn_urgent`](Self::spawn_urgent).
    /// The task is added to the high-priority queue but does not return a [`JoinHandle`].
    ///
    /// Use this for time-sensitive operations where the result is not needed.
    ///
    /// # Panics
    ///
    /// If the task panics, the panic will be caught and logged via `tracing::error!`
    /// but will not be propagated to the caller.
    pub fn spawn_urgent_and_forget<F>(&self, task: F)
    where
        F: FnOnce() + Send + 'static,
    {
        self.spawn_internal_and_forget(task, true);
    }

    /// Internal implementation of spawn.
    fn spawn_internal<R, F>(&self, task: F, urgent: bool) -> JoinHandle<R>
    where
        R: Send + 'static,
        F: FnOnce() -> R + Send + 'static,
    {
        // Capture spawn time for metrics.
        let spawn_time = CLOCK.with_borrow_mut(fast_time::Clock::now);

        let destination = self.inner.policy.destination(&self.inner.hardware);
        let state = self.inner.ensure_workers_spawned(destination);

        // Rent a oneshot channel for the result.
        let (sender, receiver) = state.result_channel_pool.rent();

        // Wrap the task to capture panics and send the result.
        let wrapped = wrap_task(task, sender, spawn_time);

        let dyn_task = allocate_task(state, wrapped);

        self.inner.enqueue(destination, state, dyn_task, urgent);

        JoinHandle::new(receiver)
    }

    /// Internal implementation of fire-and-forget spawn.
    fn spawn_internal_and_forget<F>(&self, task: F, urgent: bool)
    where
        F: FnOnce() + Send + 'static,
    {
        // Capture spawn time for metrics.
        let spawn_time = CLOCK.with_borrow_mut(fast_time::Clock::now);

        let destination = self.inner.policy.destination(&self.inner.hardware);
        let state = self.inner.ensure_workers_spawned(destination);

        // Wrap the task to capture panics and log them.
        let wrapped = wrap_task_and_forget(task, spawn_time);

        let dyn_task = allocate_task(state, wrapped);

        self.inner.enqueue(destination, state, dyn_task, urgent);
    }
}

/// Allocates and initializes a task without invoking task code under the pool lock.
fn allocate_task<T: VicinalTask>(state: &WorkerGroup, task: T) -> ErasedTaskHandle {
    let slot = {
        let pool = state.task_pool.lock().expect(NEVER_POISONED);
        pool.try_alloc_uninit_box()
    };

    let slot = match slot {
        Ok(slot) => slot,
        Err(error) => panic!("task pool allocation failed: {error}"),
    };

    init_task(slot, task)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use events_once::EventPool;
    use futures::executor::block_on;
    use many_cpus::SystemHardware;
    use many_cpus::fake::HardwareBuilder;
    use new_zealand::nz;
    use static_assertions::assert_impl_all;
    use testing::assert_panics;

    use crate::{Pool, Scheduler};

    assert_impl_all!(Scheduler: UnwindSafe, RefUnwindSafe);

    fn fake_pool() -> Pool {
        Pool::builder()
            .hardware(SystemHardware::fake(HardwareBuilder::from_counts(
                nz!(1),
                nz!(1),
            )))
            .workers_per_processor(nz!(1))
            .build()
    }

    #[test]
    fn spawn_returns_join_handle() {
        let pool = fake_pool();
        let scheduler = pool.scheduler();

        let handle = scheduler.spawn(|| 42);
        let result = block_on(handle);

        assert_eq!(result, 42);
    }

    #[test]
    fn spawn_urgent_returns_join_handle() {
        let pool = fake_pool();
        let scheduler = pool.scheduler();

        let handle = scheduler.spawn_urgent(|| "urgent result");
        let result = block_on(handle);

        assert_eq!(result, "urgent result");
    }

    #[test]
    fn scheduler_is_cloneable() {
        let pool = fake_pool();
        let scheduler = pool.scheduler();
        let scheduler2 = scheduler.clone();

        let handle1 = scheduler.spawn(|| 1);
        let handle2 = scheduler2.spawn(|| 2);

        assert_eq!(block_on(handle1), 1);
        assert_eq!(block_on(handle2), 2);
    }

    #[test]
    fn spawn_captures_panic() {
        let pool = fake_pool();
        let scheduler = pool.scheduler();

        let handle = scheduler.spawn(|| {
            panic!("test panic");
        });

        assert_panics(|| block_on(handle));
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

        let result = block_on(handle);
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
