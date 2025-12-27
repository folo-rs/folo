//! Pool management and lifecycle.

use std::any::type_name;
use std::fmt;
use std::mem;
use std::num::NonZero;
use std::panic;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::{self, JoinHandle as ThreadJoinHandle};

use event_listener::Listener;
use many_cpus::{ProcessorId, ProcessorSet};
use new_zealand::nz;
use parking_lot::Mutex;
use tracing::{debug, trace};

use crate::{IterationResult, ProcessorRegistry, Scheduler, WorkerCore};

const DEFAULT_WORKERS_PER_PROCESSOR: NonZero<u32> = nz!(2_u32);

pub(crate) struct PoolInner {
    pub(crate) registry: ProcessorRegistry,
    pub(crate) workers_per_processor: NonZero<u32>,
    pub(crate) worker_handles: Mutex<Vec<ThreadJoinHandle<()>>>,
}

impl fmt::Debug for PoolInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let handle_count = self.worker_handles.lock().len();

        f.debug_struct(type_name::<Self>())
            .field("workers_per_processor", &self.workers_per_processor)
            .field("worker_count", &handle_count)
            .finish_non_exhaustive()
    }
}

impl PoolInner {
    pub(crate) fn ensure_workers_spawned(self: &Arc<Self>, processor_id: ProcessorId) {
        let state = self.registry.get_or_init(processor_id);

        // Acquire on failure to synchronize with the Release on successful exchange.
        // AcqRel on success ensures:
        // - Acquire: we see all prior worker spawning activity.
        // - Release: subsequent worker spawning sees this flag was set.
        let already_spawned = state
            .workers_spawned
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err();

        if already_spawned {
            return;
        }

        let workers_count = self.workers_per_processor.get();
        let mut new_handles = Vec::with_capacity(workers_count as usize);

        for worker_index in 0..workers_count {
            let inner_clone = Arc::clone(self);
            let handle = thread::Builder::new()
                .name(format!("vicinal-p{processor_id}-w{worker_index}"))
                .spawn(move || {
                    // Pin worker thread to the target processor for cache locality.
                    if let Some(processor_set) = ProcessorSet::builder()
                        .filter(|p| p.id() == processor_id)
                        .take_all()
                    {
                        processor_set.pin_current_thread_to();
                    }

                    debug!(processor_id, worker_index, "worker thread started");
                    worker_loop(&inner_clone, processor_id, worker_index);
                    debug!(processor_id, worker_index, "worker thread exiting");
                })
                .expect("failed to spawn worker thread: thread spawning failure is not supported");

            new_handles.push(handle);
        }

        self.worker_handles.lock().extend(new_handles);
    }

    pub(crate) fn join_all_workers(&self) {
        self.registry.signal_shutdown_all();

        let handles = mem::take(&mut *self.worker_handles.lock());

        for handle in handles {
            if let Err(payload) = handle.join() {
                // Worker threads run inside a panic trap and should never panic. If one does,
                // something is very wrong with the pool infrastructure. We propagate the panic
                // to ensure this critical failure is not silently ignored.
                panic::resume_unwind(payload);
            }
        }
    }
}

fn worker_loop(inner: &PoolInner, processor_id: ProcessorId, worker_index: u32) {
    let state = inner.registry.get_or_init(processor_id);
    let core = WorkerCore::new(
        &state.urgent_queue,
        &state.regular_queue,
        &state.shutdown_flag,
    );

    loop {
        match core.run_one_iteration() {
            IterationResult::ExecutedUrgent => {
                trace!(processor_id, worker_index, "executed urgent task");
            }
            IterationResult::ExecutedRegular => {
                trace!(processor_id, worker_index, "executed regular task");
            }
            IterationResult::Shutdown => {
                break;
            }
            IterationResult::WaitingForWork => {
                let listener = state.wake_event.listen();

                // Re-check after registering listener to avoid lost wakeups.
                // Acquire ordering synchronizes with Release in signal_shutdown and task push.
                if !state.urgent_queue.is_empty()
                    || !state.regular_queue.is_empty()
                    || state.shutdown_flag.load(Ordering::Acquire)
                {
                    continue;
                }

                listener.wait();
            }
        }
    }
}

/// A worker pool that executes tasks on the same processor that spawned them.
///
/// This ensures optimal cache locality and minimizes cross-processor data movement.
///
/// # Lifetime
///
/// When the pool is dropped:
/// 1. All worker threads are signaled to shut down.
/// 2. The drop blocks until all currently-executing tasks complete.
/// 3. Any queued tasks that have not started are abandoned.
///
/// To ensure all tasks complete, await their [`JoinHandle`][crate::JoinHandle]s before dropping
/// the pool.
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
/// let handle = scheduler.spawn(|| 42);
/// assert_eq!(handle.await, 42);
/// # });
/// ```
#[derive(Debug)]
pub struct Pool {
    inner: Arc<PoolInner>,
}

impl Pool {
    /// Creates a new pool with default settings.
    ///
    /// Use [`Pool::builder()`] for custom configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::builder().build()
    }

    /// Creates a builder for configuring the pool.
    #[must_use]
    pub fn builder() -> PoolBuilder {
        PoolBuilder::new()
    }

    /// Returns a scheduler that can be used to spawn tasks on this pool.
    ///
    /// The scheduler can be cloned and shared across threads.
    #[must_use]
    pub fn scheduler(&self) -> Scheduler {
        Scheduler::new(Arc::clone(&self.inner))
    }
}

impl Default for Pool {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.inner.join_all_workers();
    }
}

/// Builder for configuring a [`Pool`].
#[derive(Debug)]
pub struct PoolBuilder {
    workers_per_processor: NonZero<u32>,
}

impl PoolBuilder {
    fn new() -> Self {
        Self {
            workers_per_processor: DEFAULT_WORKERS_PER_PROCESSOR,
        }
    }

    /// Sets the number of worker threads per processor.
    ///
    /// Default is 2.
    #[must_use]
    pub fn workers_per_processor(mut self, count: NonZero<u32>) -> Self {
        self.workers_per_processor = count;
        self
    }

    /// Builds the pool with the configured settings.
    #[must_use]
    pub fn build(self) -> Pool {
        let inner = Arc::new(PoolInner {
            registry: ProcessorRegistry::new(),
            workers_per_processor: self.workers_per_processor,
            worker_handles: Mutex::new(Vec::new()),
        });

        Pool { inner }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZero;

    use crate::Pool;

    #[cfg_attr(miri, ignore)]
    #[test]
    fn pool_new_creates_pool() {
        let pool = Pool::new();
        drop(pool);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn pool_builder_allows_configuration() {
        let pool = Pool::builder()
            .workers_per_processor(NonZero::new(4).unwrap())
            .build();
        drop(pool);
    }

    #[cfg_attr(miri, ignore)]
    #[test]
    fn scheduler_can_be_obtained() {
        let pool = Pool::new();
        let _scheduler = pool.scheduler();
    }
}
