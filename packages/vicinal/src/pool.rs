//! Pool configuration and ownership.

use std::any::type_name;
use std::fmt;
use std::num::NonZero;
use std::panic::{AssertUnwindSafe, catch_unwind, panic_any};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use many_cpus::{ProcessorSet, SystemHardware};
use new_zealand::nz;

use crate::policies::WorkersPerProcessor;
use crate::worker_policy::{WorkerPolicy, WorkerPolicyBinding};
use crate::{BuildError, PoolInner, Scheduler};

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
/// [`WorkerPolicy`][crate::worker_policy::WorkerPolicy] can select and create groups independently.
/// Built-in workers start on first submission unless
/// [`PoolBuilder::eager`] selects destinations for them to initialize during construction.
/// Shared workers use the initial selection plus processors included by policy selection in
/// their pool and destination, refreshing affinity at task boundaries without adding workers.
/// A pool's policy does not change after construction.
///
/// # Lifetime
///
/// Dropping the pool stops accepting work, cancels queued tasks and waits for every worker
/// participating in [`Worker::run()`][crate::worker_policy::Worker::run] to finish. Policy-owned
/// operating-system threads may perform cleanup after leaving that method; the pool does not
/// join them. Cancelled joined tasks return a
/// [`TaskError`][crate::TaskError] that retains the original body for recovery;
/// fire-and-forget tasks release their captures. Retained schedulers immediately cancel
/// submissions after clean shutdown as well.
/// Do not release the last owning `Pool` reference from a callback running on one of its workers.
/// Capture a [`Scheduler`] for follow-up submissions and retain the pool owner until
/// the required work finishes.
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
    /// built-in worker counts, or install a custom
    /// [`WorkerPolicy`][crate::worker_policy::WorkerPolicy].
    ///
    /// # Panics
    ///
    /// Panics if default pool construction fails.
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
    // The builder only exposes policy access through consuming binding, never by reference.
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

    /// Sets an identifier included in thread names created by built-in policies.
    ///
    /// The naming format and the identifier's position within it are unspecified. Custom policies
    /// can use [`BindContext::pool_name`][crate::worker_policy::BindContext::pool_name] when
    /// naming their own threads.
    #[must_use]
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Sets the policy that assigns tasks and creates worker groups.
    ///
    /// Replaces any previously selected policy; the last setter wins.
    ///
    /// Construction calls [`WorkerPolicy::bind`][crate::worker_policy::WorkerPolicy::bind] once
    /// with the final settings. All submission methods call the returned policy's
    /// [`select_group`][crate::worker_policy::BoundPolicy::select_group] when they reach routing.
    /// Submissions that observe clean shutdown before routing skip selection and do not enter
    /// the pool. Selection does not guarantee admission.
    #[must_use]
    pub fn worker_policy(mut self, policy: impl WorkerPolicy) -> Self {
        self.policy = Some(AssertUnwindSafe(Box::new(policy)));
        self
    }

    /// Selects processors for built-in worker initialization during construction.
    ///
    /// The final selection is passed to
    /// [`WorkerPolicy::bind`][crate::worker_policy::WorkerPolicy::bind] by [`Self::build`].
    /// Custom policies may ignore the selection. This setter starts no workers and takes
    /// ownership of the nonempty processor set. Repeated calls replace the selection.
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
    /// The selection must belong to the pool's
    /// [`many_cpus::SystemHardware`](https://docs.rs/many_cpus/latest/many_cpus/struct.SystemHardware.html)
    /// instance.
    ///
    /// # Example
    ///
    /// ```
    /// use many_cpus::SystemHardware;
    /// use vicinal::Pool;
    ///
    /// # fn main() -> Result<(), vicinal::BuildError> {
    /// let processors = SystemHardware::current().processors();
    /// let pool = Pool::builder().eager(processors).build()?;
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn eager(mut self, processors: ProcessorSet) -> Self {
        self.eager = Some(processors);
        self
    }

    /// Sets a function to run when each worker thread starts.
    ///
    /// The function runs once on each worker, after its initial affinity is set and before
    /// it executes any tasks. Calls on different workers may overlap, without a guaranteed
    /// order. Built-in lazy submission does not wait for initialization; [`Self::build`] waits
    /// for workers reserved during binding.
    ///
    /// The function must preserve worker affinity. It may submit work to the same pool,
    /// but must not wait for that work to complete or shut down the pool.
    /// For workers reserved during binding, the function must not depend on [`Self::build`]
    /// returning, including to publish the pool's scheduler.
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
    /// Defaults to
    /// [`many_cpus::SystemHardware::current()`](https://docs.rs/many_cpus/latest/many_cpus/struct.SystemHardware.html#method.current).
    /// A custom instance can provide simulated hardware. The pool does not refresh topology or
    /// operating-system restrictions.
    #[must_use]
    pub fn hardware(mut self, hardware: SystemHardware) -> Self {
        self.hardware = Some(hardware);
        self
    }

    /// Builds the pool with the configured worker policy and initialization.
    ///
    /// Calls [`WorkerPolicy::bind`][crate::worker_policy::WorkerPolicy::bind] with the final
    /// settings. Before returning a pool, the builder waits for each worker reserved during
    /// binding to finish initialization or for its reservation to be cancelled by dropping the
    /// unrun [`Worker`][crate::worker_policy::Worker]. This applies regardless of [`Self::eager`].
    ///
    /// # Errors
    ///
    /// Returns an error if default hardware discovery panics, the eager selection belongs to
    /// different hardware, [`WorkerPolicy::bind`][crate::worker_policy::WorkerPolicy::bind]
    /// reports a failure or worker infrastructure panics during binding or initialization.
    /// On binding or worker startup failure, the builder stops the pool and waits for
    /// participating workers to leave [`Worker::run`][crate::worker_policy::Worker::run]
    /// before returning the error; it does not join policy-owned threads.
    pub fn build(self) -> Result<Pool, BuildError> {
        let Self {
            name,
            policy,
            hardware,
            thread_init,
            eager,
        } = self;
        // Deferring the default box avoids allocating it for custom or abandoned builders.
        let policy: Box<dyn WorkerPolicyBinding> = match policy {
            Some(policy) => policy.0,
            None => Box::new(WorkersPerProcessor::new(DEFAULT_WORKERS_PER_PROCESSOR)),
        };
        let hardware = match hardware {
            Some(hardware) => hardware,
            None => contain_build_setup(|| SystemHardware::current().clone())?,
        };
        let pool_id = NEXT_POOL_ID.fetch_add(1, Ordering::Relaxed);
        let pool_name = name.unwrap_or_else(|| format!("vicinal-{pool_id}"));

        // Policy binding can start participating workers before the outer Pool exists.
        // PoolInner::new therefore owns binding failure cleanup through the runtime barrier.
        // Ref: docs/implementation.md, "Eager construction".
        Ok(Pool {
            inner: Arc::new(PoolInner::new(
                pool_id,
                pool_name,
                &hardware,
                policy,
                thread_init,
                eager.as_ref(),
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
            .field("has_policy", &self.policy.is_some())
            .field("has_thread_init", &self.thread_init.is_some())
            .field("eager", &self.eager)
            .finish_non_exhaustive()
    }
}

/// Converts hardware-discovery panics before worker participation into a fallible build result.
fn contain_build_setup<T>(setup: impl FnOnce() -> T) -> Result<T, BuildError> {
    catch_unwind(AssertUnwindSafe(setup)).map_err(BuildError::from_panic)
}

/// Shared configuration invoked only on workers, behind their pool-poisoning unwind boundary.
///
/// The callback is never exposed through a builder or scheduler's unwind boundary, so its
/// captures need not be unwind-safe. See docs/implementation.md, "Worker initialization".
pub(crate) type ThreadInit = AssertUnwindSafe<Box<dyn Fn() + Send + Sync>>;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::io::Error as IoError;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::sync::atomic::AtomicBool;

    use many_cpus::fake::HardwareBuilder;
    use ohno::ErrorExt;
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::BuildPanicked;
    use crate::worker_policy::{BindContext, BindError, BoundPolicy, WorkerGroupId, WorkerGroups};

    assert_impl_all!(Pool: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(PoolBuilder: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(PoolBuilder: Send, Sync);

    /// Detects accidental application callbacks in owner and scheduler diagnostics.
    struct PanickingDebugPolicy;

    impl fmt::Debug for PanickingDebugPolicy {
        fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
            panic!("pool diagnostics must not call the custom policy formatter");
        }
    }

    impl WorkerPolicy for PanickingDebugPolicy {
        fn bind(self, _context: BindContext<'_>) -> Result<impl BoundPolicy, BindError> {
            Ok(self)
        }
    }

    impl BoundPolicy for PanickingDebugPolicy {
        fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
            unreachable!("formatting does not submit tasks");
        }
    }

    #[test]
    fn setup_panics_become_build_errors() {
        let error = contain_build_setup(|| panic_any(IoError::other("setup failed"))).unwrap_err();

        assert!(error.find_source::<BuildPanicked>().is_some());
    }

    #[test]
    fn default_build_does_not_initialize_workers() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
        let initialized = Arc::new(AtomicBool::new(false));
        let pool = Pool::builder()
            .hardware(hardware)
            .thread_init({
                let initialized = Arc::clone(&initialized);
                // This flag reports only callback invocation, not other worker state.
                move || initialized.store(true, Ordering::Relaxed)
            })
            .build()
            .unwrap();

        assert!(!initialized.load(Ordering::Relaxed));
        drop(pool);
        assert!(!initialized.load(Ordering::Relaxed));
    }

    #[test]
    fn diagnostics_do_not_format_custom_policies() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
        let builder = Pool::builder()
            .hardware(hardware)
            .worker_policy(PanickingDebugPolicy);

        _ = format!("{builder:?}");

        let pool = builder.build().unwrap();
        let scheduler = pool.scheduler();
        _ = format!("{pool:?}");
        _ = format!("{scheduler:?}");
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
        assert!(default.inner.runtime.pool_name().starts_with("vicinal-"));
        assert_eq!(named.inner.runtime.pool_name(), "named");
    }
}
