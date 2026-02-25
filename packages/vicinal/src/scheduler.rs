//! Scheduler for spawning tasks on the worker pool.

use std::sync::Arc;

use tracing::trace;

use crate::metrics::CLOCK;
use crate::{JoinHandle, PoolInner, PooledCastVicinalTask, wrap_task};

/// A handle for spawning tasks on a [`Pool`][crate::Pool].
///
/// The scheduler determines which processor to execute tasks on based on the processor
/// that called `spawn()` or `spawn_urgent()`. This ensures cache locality by keeping
/// data close to the code that processes it.
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

    /// Spawns a task to be executed on the current processor.
    ///
    /// The task will be added to the regular (normal-priority) queue and executed
    /// by a worker thread associated with the processor that called this method.
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

    /// Spawns an urgent (high-priority) task to be executed on the current processor.
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

    /// Spawns a fire-and-forget task on the current processor.
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

    /// Spawns an urgent fire-and-forget task on the current processor.
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

        let processor_id = self.inner.hardware.current_processor_id();

        // Ensure workers are spawned for this processor (lazy initialization).
        self.inner.ensure_workers_spawned(processor_id);

        let state = self.inner.registry.get_or_init(processor_id);

        // Rent a oneshot channel for the result.
        let (sender, receiver) = state.result_channel_pool.rent();

        // Wrap the task to capture panics and send the result.
        let wrapped = wrap_task(task, sender, spawn_time);

        // Insert the wrapped task into the pool and cast to trait object.
        let pooled_task = state.task_pool.insert(wrapped);
        let dyn_task = pooled_task.cast_vicinal_task();

        // Push to the appropriate queue.
        if urgent {
            state.urgent_queue.lock().push_back(dyn_task);
            trace!(
                pool_id = self.inner.pool_id,
                processor_id, "spawned urgent task"
            );
        } else {
            state.regular_queue.lock().push_back(dyn_task);
            trace!(
                pool_id = self.inner.pool_id,
                processor_id, "spawned regular task"
            );
        }

        // Record the spawn for metrics.
        state.record_task_spawned();

        // Notify one worker that work is available.
        state.wake_event.notify(1);

        JoinHandle::new(receiver)
    }

    /// Internal implementation of fire-and-forget spawn.
    fn spawn_internal_and_forget<F>(&self, task: F, urgent: bool)
    where
        F: FnOnce() + Send + 'static,
    {
        use crate::wrap_task_and_forget;

        // Capture spawn time for metrics.
        let spawn_time = CLOCK.with_borrow_mut(fast_time::Clock::now);

        let processor_id = self.inner.hardware.current_processor_id();

        // Ensure workers are spawned for this processor (lazy initialization).
        self.inner.ensure_workers_spawned(processor_id);

        let state = self.inner.registry.get_or_init(processor_id);

        // Wrap the task to capture panics and log them.
        let wrapped = wrap_task_and_forget(task, spawn_time);

        // Insert the wrapped task into the pool and cast to trait object.
        let pooled_task = state.task_pool.insert(wrapped);
        let dyn_task = pooled_task.cast_vicinal_task();

        // Push to the appropriate queue.
        if urgent {
            state.urgent_queue.lock().push_back(dyn_task);
            trace!(
                pool_id = self.inner.pool_id,
                processor_id, "spawned urgent fire-and-forget task"
            );
        } else {
            state.regular_queue.lock().push_back(dyn_task);
            trace!(
                pool_id = self.inner.pool_id,
                processor_id, "spawned fire-and-forget task"
            );
        }

        // Record the spawn for metrics.
        state.record_task_spawned();

        // Notify one worker that work is available.
        state.wake_event.notify(1);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use events_once::EventPool;
    use futures::executor::block_on;

    use crate::Pool;

    #[test]
    fn spawn_returns_join_handle() {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        let handle = scheduler.spawn(|| 42);
        let result = block_on(handle);

        assert_eq!(result, 42);
    }

    #[test]
    fn spawn_urgent_returns_join_handle() {
        let pool = Pool::new();
        let scheduler = pool.scheduler();

        let handle = scheduler.spawn_urgent(|| "urgent result");
        let result = block_on(handle);

        assert_eq!(result, "urgent result");
    }

    #[test]
    fn scheduler_is_cloneable() {
        let pool = Pool::new();
        let scheduler = pool.scheduler();
        let scheduler2 = scheduler.clone();

        let handle1 = scheduler.spawn(|| 1);
        let handle2 = scheduler2.spawn(|| 2);

        assert_eq!(block_on(handle1), 1);
        assert_eq!(block_on(handle2), 2);
    }

    #[test]
    fn spawn_captures_panic() {
        use std::panic;

        let pool = Pool::new();
        let scheduler = pool.scheduler();

        let handle = scheduler.spawn(|| {
            panic!("test panic");
        });

        let result = panic::catch_unwind(panic::AssertUnwindSafe(|| block_on(handle)));
        assert!(result.is_err());
    }

    #[test]
    fn spawn_accepts_not_unpin_task() {
        let pool = Pool::new();
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
        let pool = Pool::new();
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
        let pool = Pool::new();
        let scheduler = pool.scheduler();
        let event_pool = EventPool::<()>::new();

        let (tx, rx) = event_pool.rent();
        scheduler.spawn_urgent_and_forget(move || {
            tx.send(());
        });

        block_on(rx).unwrap();
    }
}
