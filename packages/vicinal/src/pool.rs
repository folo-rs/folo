//! Pool configuration and ownership.

use std::num::NonZero;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use many_cpus::SystemHardware;
use new_zealand::nz;

use crate::{PoolInner, Scheduler, WorkerPolicy};

/// Experimentally determined as providing good throughput under
/// heavy I/O primitive create/destroy workload.
const DEFAULT_WORKERS_PER_PROCESSOR: NonZero<u32> = nz!(2_u32);

static NEXT_POOL_ID: AtomicU64 = AtomicU64::new(0);

/// A worker pool that executes synchronous tasks near their submitting thread.
///
/// By default, tasks execute on the processor sampled during submission.
/// [`WorkerPolicy`] can instead share a fixed worker count across each
/// submitting memory region. No workers start until the first submission to that destination.
/// Regional workers use the cumulative set of processors that submitted accepted work
/// to their pool and region, refreshing affinity at task boundaries without adding workers.
/// A pool's policy does not change after construction.
///
/// # Lifetime
///
/// Dropping the pool stops accepting work, abandons queued tasks and waits for executing tasks
/// and worker threads to finish. Abandoned tasks release their resources even if schedulers
/// remain alive. Awaiting an abandoned task's [`JoinHandle`][crate::JoinHandle] panics.
/// After clean shutdown, retained schedulers immediately abandon submissions as well.
/// An infrastructure failure stops and permanently poisons the entire pool; all further
/// submissions then panic synchronously, even after the pool is dropped.
///
/// To ensure all tasks complete, await their join handles before dropping the pool.
///
/// # Example
///
/// ```
/// use vicinal::Pool;
/// # use futures::executor::block_on;
///
/// # block_on(async {
/// let pool = Pool::new();
/// assert_eq!(pool.scheduler().spawn(|| 42).await, 42);
/// # });
/// ```
#[derive(Debug)]
pub struct Pool {
    pub(crate) inner: Arc<PoolInner>,
}

impl Pool {
    /// Creates a pool with default settings and lazily started processor-local workers.
    ///
    /// Use [`Self::builder`] and [`WorkerPolicy`] to configure worker counts explicitly.
    #[must_use]
    #[inline]
    pub fn new() -> Self {
        Self::builder().build()
    }

    /// Creates a builder for configuring the pool.
    #[must_use]
    #[inline]
    pub fn builder() -> PoolBuilder {
        PoolBuilder::new()
    }

    /// Returns a scheduler that can be cloned and shared across threads.
    #[must_use]
    #[inline]
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
    #[cfg_attr(test, mutants::skip)] // Removing shutdown strands sleeping workers.
    fn drop(&mut self) {
        self.inner.join_all_workers();
    }
}

/// Builder for configuring a [`Pool`].
#[derive(Debug)]
pub struct PoolBuilder {
    name: Option<String>,
    policy: WorkerPolicy,
    hardware: Option<SystemHardware>,
}

impl PoolBuilder {
    fn new() -> Self {
        Self {
            name: None,
            policy: WorkerPolicy::fixed_per_processor(DEFAULT_WORKERS_PER_PROCESSOR),
            hardware: None,
        }
    }

    /// Sets the name of the pool, used as a prefix in worker thread names.
    ///
    /// Processor-policy names use `{name}-p{processor_id}-{worker_index}`.
    /// Memory-region-policy names use `{name}-r{memory_region_id}-{worker_index}`.
    /// Worker indices start at zero within each destination.
    ///
    /// The default pool name is `vicinal-{N}`, where `N` is an auto-incrementing pool
    /// counter that distinguishes different pools.
    ///
    /// The default naming scheme is not part of the API contract and may change at any time.
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Sets the number of worker threads per processor.
    ///
    /// Equivalent to `worker_policy(WorkerPolicy::fixed_per_processor(count))`.
    /// This replaces any previously selected policy; the last policy setter wins.
    #[doc(hidden)]
    #[deprecated(note = "use worker_policy(WorkerPolicy::fixed_per_processor(count)) instead")]
    #[must_use]
    #[inline]
    pub fn workers_per_processor(self, count: NonZero<u32>) -> Self {
        self.worker_policy(WorkerPolicy::fixed_per_processor(count))
    }

    /// Sets the pool's worker count and locality policy.
    ///
    /// Replaces any previously selected policy; the last setter wins.
    #[must_use]
    pub fn worker_policy(mut self, policy: WorkerPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Sets the hardware instance used for topology and locality.
    ///
    /// Defaults to [`SystemHardware::current()`]. A custom instance can provide simulated
    /// hardware. The pool does not refresh topology or operating-system restrictions.
    #[must_use]
    pub fn hardware(mut self, hardware: SystemHardware) -> Self {
        self.hardware = Some(hardware);
        self
    }

    /// Builds the pool without starting worker threads.
    #[must_use]
    pub fn build(self) -> Pool {
        let hardware = self
            .hardware
            .unwrap_or_else(|| SystemHardware::current().clone());
        let pool_id = NEXT_POOL_ID.fetch_add(1, Ordering::Relaxed);
        let pool_name = self.name.unwrap_or_else(|| format!("vicinal-{pool_id}"));

        Pool {
            inner: Arc::new(PoolInner::new(pool_id, pool_name, hardware, self.policy)),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use many_cpus::fake::HardwareBuilder;
    use static_assertions::assert_impl_all;

    use super::*;
    use crate::GroupId;

    assert_impl_all!(Pool: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(PoolBuilder: UnwindSafe, RefUnwindSafe);

    #[test]
    #[expect(
        deprecated,
        reason = "verifies compatibility and precedence of the legacy setter"
    )]
    fn default_and_policy_setter_order() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
        let default = Pool::builder().hardware(hardware.clone()).build();
        let region = Pool::builder()
            .hardware(hardware.clone())
            .workers_per_processor(nz!(3))
            .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(1)))
            .build();
        let processor = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(WorkerPolicy::fixed_per_memory_region(nz!(1)))
            .workers_per_processor(nz!(3))
            .build();

        for (pool, destination, count) in [
            (&default, GroupId::Processor(0), nz!(2)),
            (&region, GroupId::MemoryRegion(0), nz!(1)),
            (&processor, GroupId::Processor(0), nz!(3)),
        ] {
            assert_eq!(pool.inner.policy.destination(&hardware).group, destination);
            assert_eq!(pool.inner.policy.count(), count);
            assert_eq!(pool.inner.registry.initialized().count(), 0);
            assert!(pool.inner.worker_handles.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn pool_names() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
        let default = Pool::builder().hardware(hardware.clone()).build();
        let named = Pool::builder().hardware(hardware).name("named").build();
        assert!(default.inner.pool_name.starts_with("vicinal-"));
        assert_eq!(named.inner.pool_name, "named");
    }
}
