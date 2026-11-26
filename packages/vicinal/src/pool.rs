//! Pool configuration and ownership.

use std::any::type_name;
use std::fmt;
use std::num::NonZero;
use std::panic::{AssertUnwindSafe, panic_any};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use many_cpus::{ProcessorSet, SystemHardware};
use new_zealand::nz;

use crate::policies::WorkersPerProcessor;
#[cfg(test)]
use crate::policies::{WorkersPerMemoryRegion, WorkersPerSystem};
use crate::{PoolInner, PrepareError, Scheduler, WorkerPolicy, WorkerPolicyBinding};

/// Experimentally determined as providing good throughput under
/// heavy I/O primitive create/destroy workload.
const DEFAULT_WORKERS_PER_PROCESSOR: NonZero<u32> = nz!(2_u32);

static NEXT_POOL_ID: AtomicU64 = AtomicU64::new(0);

/// A synchronous worker pool with configurable processor affinity.
///
/// By default, tasks execute on the processor sampled during submission.
/// [`policies::WorkersPerMemoryRegion`][crate::policies::WorkersPerMemoryRegion] and
/// [`policies::WorkersPerSystem`][crate::policies::WorkersPerSystem] can instead share a fixed
/// worker count across each submitting memory region or across the system. A custom
/// [`WorkerPolicy`] can select and create groups independently. Built-in workers start on first
/// submission unless
/// [`PoolBuilder::eager`] initializes their destinations during construction.
/// Shared workers use the initial selection plus processors included by policy selection in
/// their pool and destination, refreshing affinity at task boundaries without adding workers.
/// A pool's policy does not change after construction.
///
/// # Lifetime
///
/// Dropping the pool stops accepting work, cancels queued tasks and waits for every worker
/// participating in [`Worker::run()`][crate::Worker::run] to finish. Policy-owned
/// operating-system threads may perform cleanup after leaving that method; the pool does not
/// join them. Cancelled joined tasks return a
/// [`TaskError`][crate::TaskError] that retains the original body for recovery;
/// fire-and-forget tasks release their captures. Retained schedulers immediately cancel
/// submissions after clean shutdown as well.
/// Do not release the last owning `Pool` reference from a callback running on one of its workers;
/// the worker cannot leave the pool while shutdown waits for it. Capture a [`Scheduler`] for
/// follow-up submissions and retain the pool owner until the required work finishes.
/// An infrastructure failure stops and permanently poisons the entire pool; all further
/// submissions then panic synchronously, even after the pool is dropped.
///
/// To ensure all tasks complete, await their join handles before dropping the pool.
///
/// # Example
///
/// ```
/// # use std::error::Error;
/// use vicinal::Pool;
/// # use futures::executor::block_on;
///
/// # block_on(async {
/// let pool = Pool::new();
/// assert_eq!(pool.scheduler().spawn(|| 42).await?, 42);
/// # Ok::<(), Box<dyn Error>>(())
/// # }).unwrap();
/// ```
#[derive(Debug)]
pub struct Pool {
    pub(crate) inner: Arc<PoolInner>,
}

impl Pool {
    /// Creates a pool with default settings and lazily started processor-local workers.
    ///
    /// Use [`Self::builder`] with one of the [`policies`][crate::policies] types to configure
    /// built-in worker counts, or install a custom [`WorkerPolicy`].
    ///
    /// # Panics
    ///
    /// Panics if the default policy cannot finish binding.
    #[must_use]
    #[inline]
    pub fn new() -> Self {
        Self::builder()
            .build()
            .unwrap_or_else(|error| panic_any(error))
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
        self.inner.shutdown_and_wait_for_workers();
    }
}

/// Collects pool configuration before one fallible binding phase.
///
/// The builder chooses the concrete policy, topology and initialization callback before
/// construction, so binding starts with the final immutable configuration and can return
/// a policy error before publishing a usable pool.
pub struct PoolBuilder {
    name: Option<String>,
    policy: Option<AssertUnwindSafe<Box<dyn WorkerPolicyBinding>>>,
    hardware: Option<SystemHardware>,
    thread_init: Option<ThreadInit>,
    eager: Option<ProcessorSet>,
}

impl PoolBuilder {
    fn new() -> Self {
        Self {
            name: None,
            policy: None,
            hardware: None,
            thread_init: None,
            eager: None,
        }
    }

    /// Sets an identifier included in worker thread names.
    ///
    /// The naming format and the identifier's position within it are unspecified.
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Sets the policy that assigns tasks and creates worker groups.
    ///
    /// Replaces any previously selected policy; the last setter wins.
    ///
    /// The builder calls [`WorkerPolicy::bind`] once with the final settings. Subsequent
    /// submissions call the returned [`BoundWorkerPolicy::select_group`][crate::BoundWorkerPolicy::select_group].
    #[must_use]
    pub fn worker_policy(mut self, policy: impl WorkerPolicy) -> Self {
        self.policy = Some(AssertUnwindSafe(Box::new(policy)));
        self
    }

    /// Initializes workers for the selected processors during construction.
    ///
    /// The final selection is passed to [`WorkerPolicy::bind`]. A custom policy reserves its
    /// required workers with [`WorkerGroup::add_worker`][crate::WorkerGroup::add_worker].
    /// The builder waits after `bind()` returns for each reserved worker to finish
    /// initialization; dropping an unrun [`Worker`][crate::Worker] cancels that reservation.
    /// The setter itself starts no workers and takes ownership of the nonempty processor set.
    /// Repeated calls replace the selection; construction uses the final builder settings.
    ///
    /// With [`policies::WorkersPerProcessor`][crate::policies::WorkersPerProcessor], each selected
    /// processor gets its configured worker count. With
    /// [`policies::WorkersPerMemoryRegion`][crate::policies::WorkersPerMemoryRegion], each
    /// represented region gets its count, with affinity limited to selected processors in that
    /// region. [`policies::WorkersPerSystem`][crate::policies::WorkersPerSystem] starts its count
    /// once, with affinity covering the entire selection. The constructing thread's affinity
    /// remains unchanged.
    ///
    /// This is an initial selection, not a permanent restriction. Later submissions from other
    /// eligible processors start missing destinations lazily or expand shared-worker affinity.
    /// The selection must belong to the pool's [`SystemHardware`] instance.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    /// use vicinal::Pool;
    ///
    /// let processors = SystemHardware::current().processors();
    /// let pool = Pool::builder().eager(processors).build().unwrap();
    /// ```
    #[must_use]
    pub fn eager(mut self, processors: ProcessorSet) -> Self {
        self.eager = Some(processors);
        self
    }

    /// Sets a function to run when each worker thread starts.
    ///
    /// The function runs once on each worker, after its initial affinity is set and before
    /// it executes any tasks. Built-in workers start lazily unless [`Self::eager`] initializes
    /// selected destinations during construction; a custom [`WorkerPolicy`] controls its own
    /// startup. After a custom policy's [`WorkerPolicy::bind`] returns, the builder waits
    /// for every worker reserved during binding to finish initialization or to be cancelled
    /// by dropping its unrun value. Calls on different workers may overlap, without a guaranteed
    /// order. Built-in lazy submission does not wait for initialization.
    ///
    /// The function must preserve worker affinity. It may submit work to the same pool,
    /// but must not wait for that work to complete or shut down the pool.
    /// During eager construction, it must not depend on the constructing thread returning from
    /// [`Self::build`], including to publish the pool's scheduler.
    /// A panic is treated as a [worker failure][Scheduler#worker-failures].
    ///
    /// By default, no initialization function is configured. Repeated calls replace the function.
    ///
    /// # Example
    ///
    /// ```
    /// use std::cell::Cell;
    ///
    /// # use std::error::Error;
    /// use vicinal::Pool;
    /// # use futures::executor::block_on;
    ///
    /// thread_local! {
    ///     static WORKER_LABEL: Cell<&'static str> = const { Cell::new("uninitialized") };
    /// }
    ///
    /// # block_on(async {
    /// let pool = Pool::builder()
    ///     .thread_init(|| WORKER_LABEL.set("background"))
    ///     .build()?;
    ///
    /// assert_eq!(
    ///     pool.scheduler().spawn(|| WORKER_LABEL.get()).await?,
    ///     "background"
    /// );
    /// # Ok::<(), Box<dyn Error>>(())
    /// # }).unwrap();
    /// ```
    #[must_use]
    // Omitting the callback can strand tasks and tests that synchronize with initialization.
    #[cfg_attr(test, mutants::skip)]
    pub fn thread_init(mut self, thread_init: impl Fn() + Send + Sync + 'static) -> Self {
        self.thread_init = Some(AssertUnwindSafe(Box::new(thread_init)));
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

    /// Builds the pool with the configured worker policy and initialization.
    ///
    /// The default policy starts no workers during construction unless [`Self::eager`] selects
    /// eager destinations. A custom policy may create or wait for workers during `bind()`.
    ///
    /// # Errors
    ///
    /// Returns an error reported by [`WorkerPolicy::bind`] after all participating workers
    /// started during binding have left the worker loop.
    pub fn build(self) -> Result<Pool, PrepareError> {
        let hardware = self
            .hardware
            .unwrap_or_else(|| SystemHardware::current().clone());
        let pool_id = NEXT_POOL_ID.fetch_add(1, Ordering::Relaxed);
        let pool_name = self.name.unwrap_or_else(|| format!("vicinal-{pool_id}"));
        // Deferring the default box avoids allocating it for custom or abandoned builders.
        let policy: Box<dyn WorkerPolicyBinding> = match self.policy {
            Some(policy) => policy.0,
            None => Box::new(WorkersPerProcessor::new(DEFAULT_WORKERS_PER_PROCESSOR)),
        };

        // Policy binding can start participating workers before the outer Pool exists.
        // PoolInner::new therefore owns binding failure cleanup through the runtime barrier.
        // Ref: docs/implementation.md, "Eager construction".
        Ok(Pool {
            inner: Arc::new(PoolInner::new(
                pool_id,
                pool_name,
                &hardware,
                policy,
                self.thread_init,
                self.eager.as_ref(),
            )?),
        })
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for PoolBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("name", &self.name)
            .field("hardware", &self.hardware)
            .field("policy", &self.policy)
            .field("has_thread_init", &self.thread_init.is_some())
            .field("eager", &self.eager)
            .finish_non_exhaustive()
    }
}

/// Shared configuration invoked only on workers, behind their pool-poisoning unwind boundary.
///
/// The callback is never exposed through a builder or scheduler's unwind boundary, so its
/// captures need not be unwind-safe. See docs/implementation.md, "Worker initialization".
pub(crate) type ThreadInit = AssertUnwindSafe<Box<dyn Fn() + Send + Sync>>;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use many_cpus::fake::HardwareBuilder;
    use static_assertions::assert_impl_all;

    use super::*;
    assert_impl_all!(Pool: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(PoolBuilder: UnwindSafe, RefUnwindSafe);

    #[test]
    fn default_and_policy_setter_order() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
        let default = Pool::builder().hardware(hardware.clone()).build().unwrap();
        let region = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(WorkersPerProcessor::new(nz!(3)))
            .worker_policy(WorkersPerMemoryRegion::new(nz!(1)))
            .build()
            .unwrap();
        let processor = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(WorkersPerSystem::new(nz!(1)))
            .worker_policy(WorkersPerProcessor::new(nz!(3)))
            .build()
            .unwrap();
        let system = Pool::builder()
            .hardware(hardware)
            .worker_policy(WorkersPerMemoryRegion::new(nz!(1)))
            .worker_policy(WorkersPerSystem::new(nz!(3)))
            .build()
            .unwrap();

        for pool in [&default, &region, &processor, &system] {
            assert_eq!(pool.inner.registry.initialized().count(), 0);
            assert_eq!(pool.inner.runtime.active_workers(), 0);
        }
    }

    #[test]
    fn pool_names() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
        let default = Pool::builder().hardware(hardware.clone()).build().unwrap();
        let named = Pool::builder()
            .hardware(hardware)
            .name("named")
            .build()
            .unwrap();
        assert!(default.inner.pool_name.starts_with("vicinal-"));
        assert_eq!(named.inner.pool_name, "named");
    }
}
