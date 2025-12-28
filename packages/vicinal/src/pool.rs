//! Pool management and lifecycle.

use std::any::type_name;
use std::num::NonZero;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread::{self, JoinHandle as ThreadJoinHandle};
use std::{fmt, mem, panic};

use event_listener::{Listener, listener};
use many_cpus::{ProcessorId, ProcessorSet};
use new_zealand::nz;
use parking_lot::Mutex;
use tracing::{debug, trace};

use crate::{IterationResult, ProcessorRegistry, Scheduler, WorkerCore};

/// Experimentally determined as providing good throughput under
/// heavy I/O primitive create/destroy workload.
const DEFAULT_WORKERS_PER_PROCESSOR: NonZero<u32> = nz!(2_u32);

static NEXT_POOL_ID: AtomicU64 = AtomicU64::new(0);

pub(crate) struct PoolInner {
    pub(crate) pool_id: u64,
    pub(crate) registry: ProcessorRegistry,
    pub(crate) workers_per_processor: NonZero<u32>,
    pub(crate) worker_handles: Mutex<Vec<ThreadJoinHandle<()>>>,
    pub(crate) shutdown: AtomicBool,
}

impl fmt::Debug for PoolInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let handle_count = self.worker_handles.lock().len();

        f.debug_struct(type_name::<Self>())
            .field("pool_id", &self.pool_id)
            .field("workers_per_processor", &self.workers_per_processor)
            .field("worker_count", &handle_count)
            .finish_non_exhaustive()
    }
}

impl PoolInner {
    #[cfg_attr(test, mutants::skip)] // Removing this causes timeouts (workers never start)
    pub(crate) fn ensure_workers_spawned(self: &Arc<Self>, processor_id: ProcessorId) {
        // If the pool is shutting down, we should not spawn new workers.
        if self.shutdown.load(Ordering::Relaxed) {
            return;
        }

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

                    debug!(
                        pool_id = inner_clone.pool_id,
                        processor_id, worker_index, "worker thread started"
                    );
                    worker_loop(&inner_clone, processor_id, worker_index);
                    debug!(
                        pool_id = inner_clone.pool_id,
                        processor_id, worker_index, "worker thread exiting"
                    );
                })
                .expect("failed to spawn worker thread: thread spawning failure is not supported");

            new_handles.push(handle);
        }

        let mut worker_handles = self.worker_handles.lock();

        // Re-check shutdown flag under the lock to avoid race condition where
        // join_all_workers() runs concurrently and we add new handles after it
        // has already taken the existing ones.
        //
        // If shutdown is true here, it means join_all_workers() has already
        // set the flag (and possibly taken the handles). We must not add our
        // new handles to the list, as they would be leaked. Instead, we join
        // them immediately.
        if self.shutdown.load(Ordering::Acquire) {
            for handle in new_handles {
                if let Err(payload) = handle.join() {
                    panic::resume_unwind(payload);
                }
            }
            return;
        }

        worker_handles.extend(new_handles);
    }

    pub(crate) fn join_all_workers(&self) {
        // Signal shutdown to prevent new workers from being spawned.
        // We use Release to ensure this store is visible to ensure_workers_spawned
        // when it acquires the lock.
        self.shutdown.store(true, Ordering::Release);

        // Signal all existing workers to exit.
        self.registry.signal_shutdown_all();

        // We take the handles out of the mutex, ensuring that no other thread can
        // access them. Because we set the shutdown flag above, we know that
        // ensure_workers_spawned() will not add any new handles to the list after
        // we release the lock (or if it does, it will see the shutdown flag and
        // return early).
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

#[cfg_attr(test, mutants::skip)] // Removing this causes timeouts; condition logic is race-sensitive
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
                trace!(
                    pool_id = inner.pool_id,
                    processor_id, worker_index, "executed urgent task"
                );
            }
            IterationResult::ExecutedRegular => {
                trace!(
                    pool_id = inner.pool_id,
                    processor_id, worker_index, "executed regular task"
                );
            }
            IterationResult::Shutdown => {
                break;
            }
            IterationResult::WaitingForWork => {
                listener!(state.wake_event => listener);

                // Re-check after registering listener to avoid lost wakeups.
                // Acquire ordering synchronizes with Release in signal_shutdown and task push.
                if !state.urgent_queue.lock().is_empty()
                    || !state.regular_queue.lock().is_empty()
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
    #[must_use]
    pub fn workers_per_processor(mut self, count: NonZero<u32>) -> Self {
        self.workers_per_processor = count;
        self
    }

    /// Builds the pool with the configured settings.
    #[must_use]
    pub fn build(self) -> Pool {
        let inner = Arc::new(PoolInner {
            pool_id: NEXT_POOL_ID.fetch_add(1, Ordering::Relaxed),
            registry: ProcessorRegistry::new(),
            workers_per_processor: self.workers_per_processor,
            worker_handles: Mutex::new(Vec::new()),
            shutdown: AtomicBool::new(false),
        });

        Pool { inner }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZero;
    use std::sync::mpsc;

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

    #[cfg_attr(miri, ignore)]
    #[test]
    fn drop_waits_for_workers() {
        let pool = Pool::new();
        let scheduler = pool.scheduler();
        let (tx, rx) = mpsc::channel();

        scheduler.spawn(move || {
            tx.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(50));
        });

        // Wait for task to start.
        rx.recv().unwrap();

        let start = std::time::Instant::now();
        drop(pool);

        // If drop didn't wait, this would be near-instant.
        assert!(start.elapsed() >= std::time::Duration::from_millis(50));
    }
}
