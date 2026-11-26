use std::any::type_name;
use std::error::Error;
use std::fmt::{Debug, Formatter, Result as FmtResult};
use std::panic::{RefUnwindSafe, UnwindSafe};

use many_cpus::{ProcessorSet, SystemHardware};
use ohno::OhnoCore;

use crate::{WorkerGroupId, WorkerGroups};
/// Defines one pool's task routing and policy-owned worker topology before pool binding.
///
/// Binding consumes this configuration and returns a separate [`BoundWorkerPolicy`] owned by
/// the completed pool. This keeps configuration-time setup distinct from the policy value used
/// for subsequent task routing.
///
/// A policy reserves a worker with
/// [`WorkerGroup::add_worker`][crate::WorkerGroup::add_worker], moves the returned
/// [`Worker`][crate::Worker] into a thread, applies its intended affinity and calls
/// [`Worker::run`][crate::Worker::run]. That method blocks until the worker leaves the pool.
/// `Debug` lets builders and pools identify the configured policy. The unbound configuration is
/// consumed during construction, so it needs none of the thread-safety contracts required by
/// the value returned from [`bind`][Self::bind]. The static lifetime lets the builder erase the
/// concrete configuration until its one binding call.
///
/// # Example
///
/// ```
/// # use std::error::Error;
/// use std::thread::spawn;
///
/// use vicinal::{
///     BindContext, BindError, BoundWorkerPolicy, Pool, WorkerGroupId, WorkerGroups, WorkerPolicy,
/// };
/// # use futures::executor::block_on;
///
/// #[derive(Debug, Default)]
/// struct OneGroupPolicy {
///     group: Option<WorkerGroupId>,
/// }
///
/// impl WorkerPolicy for OneGroupPolicy {
///     fn bind(mut self, context: BindContext<'_>) -> Result<impl BoundWorkerPolicy, BindError> {
///         let group = context.groups().add();
///         spawn({
///             let worker = group.add_worker();
///             move || worker.run()
///         });
///         self.group = Some(group.id());
///         Ok(self)
///     }
/// }
///
/// impl BoundWorkerPolicy for OneGroupPolicy {
///     fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
///         self.group.expect("binding installs the policy state once")
///     }
/// }
///
/// # block_on(async {
/// let pool = Pool::builder()
///     .worker_policy(OneGroupPolicy::default())
///     .build()?;
/// assert_eq!(pool.scheduler().spawn(|| 42).await?, 42);
/// # Ok::<(), Box<dyn Error>>(())
/// # }).unwrap();
/// ```
pub trait WorkerPolicy: Debug + 'static {
    /// Consumes the configuration and returns the policy used for task routing.
    ///
    /// A policy creates binding-time workers with
    /// [`WorkerGroup::add_worker`][crate::WorkerGroup::add_worker] and may clone
    /// the supplied [`WorkerGroups`] to support later asynchronous group creation. The builder
    /// counts those reservations and waits for each worker to complete its initialization
    /// callback after this method returns. If the policy cannot run a reserved worker, dropping
    /// it cancels that target.
    fn bind(self, context: BindContext<'_>) -> Result<impl BoundWorkerPolicy, BindError>
    where
        Self: Sized;
}

/// Selects task destinations after a [`WorkerPolicy`] binds to one pool.
///
/// This type is shared by schedulers, so `Send + Sync` make concurrent selection part of its
/// contract. The `Pool` public API preserves its unwind-safety contract across shutdown and
/// submission boundaries, so bound policies are also `UnwindSafe + RefUnwindSafe`. `Debug`
/// exposes the active policy in pool diagnostics. Selection should keep the established routing
/// path lock-free and avoid cloning [`WorkerGroup`][crate::WorkerGroup] handles; group creation
/// and worker startup may clone handles because they occur off that path.
pub trait BoundWorkerPolicy: Debug + Send + Sync + UnwindSafe + RefUnwindSafe + 'static {
    /// Selects the group for one submitted task.
    ///
    /// The returned ID is pool-specific and copied into the scheduler's enqueue path
    /// without cloning a worker group. The first selection may create groups or workers;
    /// established selection should remain on the ordinary task fast path. Policies that
    /// retain a [`WorkerGroup`][crate::WorkerGroup] can use
    /// [`WorkerGroup::queue_len`][crate::WorkerGroup::queue_len] to observe existing
    /// backlog, but selection runs before the incoming task is added to that queue.
    fn select_group(&self, groups: &WorkerGroups) -> WorkerGroupId;
}

/// Erases an unbound policy while preserving a by-value binding operation.
pub(crate) trait WorkerPolicyBinding: Debug {
    fn bind(
        self: Box<Self>,
        context: BindContext<'_>,
    ) -> Result<Box<dyn BoundWorkerPolicy>, BindError>;
}

impl<T> WorkerPolicyBinding for T
where
    T: WorkerPolicy,
{
    fn bind(
        self: Box<Self>,
        context: BindContext<'_>,
    ) -> Result<Box<dyn BoundWorkerPolicy>, BindError> {
        let bound = (*self).bind(context)?;
        Ok(Box::new(bound))
    }
}

/// A worker policy failed while binding to a pool.
///
/// The error retains the policy's original failure in its source chain.
#[derive(ohno::Error)]
#[no_constructors]
#[display("worker policy binding failed")]
pub struct BindError {
    #[error]
    core: OhnoCore,
}

impl UnwindSafe for BindError {}
impl RefUnwindSafe for BindError {}

impl BindError {
    /// Creates a binding error from a policy-specific failure.
    #[must_use]
    pub fn new(error: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        Self {
            core: OhnoCore::from(error),
        }
    }
}

/// Final pool configuration supplied to [`WorkerPolicy::bind`].
#[derive(Clone, Copy)]
pub struct BindContext<'a> {
    hardware: &'a SystemHardware,
    eager: Option<&'a ProcessorSet>,
    groups: &'a WorkerGroups,
    pool_name: &'a str,
}

impl Debug for BindContext<'_> {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct(type_name::<Self>())
            .field("pool_name", &self.pool_name)
            .field("eager_processor_count", &self.eager.map(ProcessorSet::len))
            .field("worker_group_count", &self.groups.len())
            .field(
                "hardware_processor_count",
                &self.hardware.all_processors().len(),
            )
            .finish()
    }
}

impl BindContext<'_> {
    pub(crate) fn new<'a>(
        hardware: &'a SystemHardware,
        eager: Option<&'a ProcessorSet>,
        groups: &'a WorkerGroups,
        pool_name: &'a str,
    ) -> BindContext<'a> {
        BindContext {
            hardware,
            eager,
            groups,
            pool_name,
        }
    }

    /// Returns the pool's fixed topology and hardware instance.
    #[must_use]
    #[inline]
    pub fn hardware(&self) -> &SystemHardware {
        self.hardware
    }

    /// Returns the optional processor selection configured for eager construction.
    #[must_use]
    #[inline]
    pub fn eager(&self) -> Option<&ProcessorSet> {
        self.eager
    }

    /// Returns this pool's worker-group collection.
    #[must_use]
    #[inline]
    pub fn groups(&self) -> &WorkerGroups {
        self.groups
    }

    /// Returns the identifier included in built-in worker thread names.
    #[must_use]
    #[inline]
    pub fn pool_name(&self) -> &str {
        self.pool_name
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use many_cpus::SystemHardware;
    use many_cpus::fake::HardwareBuilder;
    use new_zealand::nz;
    use static_assertions::assert_impl_all;

    use super::*;
    use crate::Pool;

    assert_impl_all!(
        BindContext<'static>: Copy,
        Send,
        Sync,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(BindError: Send, Sync, UnwindSafe, RefUnwindSafe);

    #[test]
    fn preparation_context_formats() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
        let eager = hardware.all_processors();
        let pool = Pool::builder().hardware(hardware.clone()).build().unwrap();
        let context = BindContext::new(&hardware, Some(&eager), &pool.inner.groups, "formatting");

        _ = format!("{context:?}");
    }
}
