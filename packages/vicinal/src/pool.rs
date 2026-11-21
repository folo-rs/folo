//! Pool configuration and ownership.

use std::any::type_name;
use std::fmt;
use std::num::NonZero;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use many_cpus::{ProcessorSet, SystemHardware};
use new_zealand::nz;

use crate::{DefaultWorkerPolicy, PoolInner, Scheduler, WorkerPolicy};

/// Experimentally determined as providing good throughput under
/// heavy I/O primitive create/destroy workload.
const DEFAULT_WORKERS_PER_PROCESSOR: NonZero<u32> = nz!(2_u32);

static NEXT_POOL_ID: AtomicU64 = AtomicU64::new(0);

/// A synchronous worker pool with configurable processor affinity.
///
/// By default, tasks execute on the processor sampled during submission.
/// [`DefaultWorkerPolicy`] can instead share a fixed worker count across each
/// submitting memory region or across the system. A custom [`WorkerPolicy`] can select and
/// create groups independently. Default-policy workers start on first submission unless
/// [`PoolBuilder::eager`] initializes their destinations during construction.
/// Shared workers use the initial selection plus processors that submitted accepted work
/// to their pool and destination, refreshing affinity at task boundaries without adding workers.
/// A pool's policy does not change after construction.
///
/// # Lifetime
///
/// Dropping the pool stops accepting work, cancels queued tasks and waits for executing tasks
/// and worker threads to finish. Cancelled joined tasks return a
/// [`TaskError`][crate::TaskError] that retains the original body for recovery;
/// fire-and-forget tasks release their captures. Retained schedulers immediately cancel
/// submissions after clean shutdown as well.
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
    /// Use [`Self::builder`] and [`DefaultWorkerPolicy`] to configure default worker counts,
    /// or install a custom [`WorkerPolicy`].
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
pub struct PoolBuilder {
    name: Option<String>,
    policy: Box<dyn WorkerPolicy>,
    hardware: Option<SystemHardware>,
    thread_init: Option<ThreadInit>,
    eager: Option<ProcessorSet>,
}

impl PoolBuilder {
    fn new() -> Self {
        Self {
            name: None,
            policy: Box::new(DefaultWorkerPolicy::per_processor(
                DEFAULT_WORKERS_PER_PROCESSOR,
            )),
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
    /// Replaces any previously selected policy; the last setter wins. Policies are boxed
    /// internally, so `Pool` and `Scheduler` remain non-generic.
    ///
    /// The builder calls [`WorkerPolicy::prepare`] once with the final settings. Subsequent
    /// submissions call [`WorkerPolicy::select_group`].
    #[must_use]
    pub fn worker_policy(mut self, policy: impl WorkerPolicy) -> Self {
        self.policy = Box::new(policy);
        self
    }

    /// Initializes workers for the selected processors during construction.
    ///
    /// The final selection is passed to [`WorkerPolicy::prepare`]. A custom policy decides how
    /// it maps the selection to groups and is responsible for waiting for required worker
    /// readiness before preparation returns. The setter itself starts no workers and takes
    /// ownership of the nonempty processor set. Repeated calls replace the selection;
    /// construction uses the final builder settings.
    ///
    /// With a per-processor default policy, each selected processor gets its configured worker
    /// count. With a regional default policy, each represented region gets its count, with
    /// affinity limited to selected processors in that region. A system default policy starts
    /// its count once, with affinity covering the entire selection. The constructing thread's
    /// affinity remains unchanged.
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
    /// let pool = Pool::builder().eager(processors).build();
    /// ```
    #[must_use]
    pub fn eager(mut self, processors: ProcessorSet) -> Self {
        self.eager = Some(processors);
        self
    }

    /// Sets a function to run when each worker thread starts.
    ///
    /// The function runs once on each worker, after its initial affinity is set and before
    /// it executes any tasks. Workers start lazily by default; [`Self::eager`] initializes selected
    /// workers during construction. Calls on different workers may overlap, without a guaranteed
    /// order. Submission does not wait for initialization; eager construction does.
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
    ///     .build();
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
    /// By default, construction starts no workers. With [`Self::eager`], it waits for the selected
    /// workers' affinity setup and initialization. Other destinations remain lazy.
    ///
    /// # Panics
    ///
    /// Panics if eager worker creation, affinity setup or initialization fails.
    /// Started workers are joined before the panic propagates.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Omitting eager startup can strand initializer coordination.
    pub fn build(self) -> Pool {
        let hardware = self
            .hardware
            .unwrap_or_else(|| SystemHardware::current().clone());
        let pool_id = NEXT_POOL_ID.fetch_add(1, Ordering::Relaxed);
        let pool_name = self.name.unwrap_or_else(|| format!("vicinal-{pool_id}"));

        // Establish the shutdown owner before spawning: workers retain their own inner references.
        // Ref: docs/implementation.md, "Eager construction".
        Pool {
            inner: Arc::new(PoolInner::new(
                pool_id,
                pool_name,
                &hardware,
                self.policy,
                self.thread_init,
                self.eager.as_ref(),
            )),
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for PoolBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let policy = std::any::type_name_of_val(&*self.policy);
        f.debug_struct(type_name::<Self>())
            .field("name", &self.name)
            .field("has_worker_policy", &true)
            .field("hardware", &self.hardware)
            .field("has_thread_init", &self.thread_init.is_some())
            .field("eager", &self.eager)
            .field("policy", &policy)
            .finish()
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
        let default = Pool::builder().hardware(hardware.clone()).build();
        let region = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(DefaultWorkerPolicy::per_processor(nz!(3)))
            .worker_policy(DefaultWorkerPolicy::per_memory_region(nz!(1)))
            .build();
        let processor = Pool::builder()
            .hardware(hardware.clone())
            .worker_policy(DefaultWorkerPolicy::per_system(nz!(1)))
            .worker_policy(DefaultWorkerPolicy::per_processor(nz!(3)))
            .build();
        let system = Pool::builder()
            .hardware(hardware)
            .worker_policy(DefaultWorkerPolicy::per_memory_region(nz!(1)))
            .worker_policy(DefaultWorkerPolicy::per_system(nz!(3)))
            .build();

        for pool in [&default, &region, &processor, &system] {
            assert_eq!(pool.inner.registry.initialized().count(), 0);
            assert_eq!(pool.inner.runtime.active_workers(), 0);
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
