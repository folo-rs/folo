use std::fmt::Debug;
use std::panic::{RefUnwindSafe, UnwindSafe};

use crate::worker_policy::{BindContext, BindError, WorkerGroupId, WorkerGroups};

/// Defines one pool's task routing and policy-owned worker topology before pool binding.
///
/// Binding consumes this configuration and returns a separate [`BoundPolicy`] owned by
/// the completed pool. This keeps configuration-time setup distinct from the policy value used
/// for subsequent task routing.
///
/// A policy reserves a worker with
/// [`WorkerGroup::add_worker`][crate::worker_policy::WorkerGroup::add_worker], moves the returned
/// [`Worker`][crate::worker_policy::Worker] into a thread, applies its intended affinity and calls
/// [`Worker::run`][crate::worker_policy::Worker::run]. That method blocks until the worker leaves
/// the pool.
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
/// use vicinal::Pool;
/// use vicinal::worker_policy::{
///     BindContext, BindError, BoundPolicy, WorkerGroupId, WorkerGroups, WorkerPolicy,
/// };
/// # use futures::executor::block_on;
///
/// #[derive(Debug, Default)]
/// struct OneGroupPolicy {
///     group: Option<WorkerGroupId>,
/// }
///
/// impl WorkerPolicy for OneGroupPolicy {
///     fn bind(mut self, context: BindContext<'_>) -> Result<impl BoundPolicy, BindError> {
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
/// impl BoundPolicy for OneGroupPolicy {
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
    /// [`WorkerGroup::add_worker`][crate::worker_policy::WorkerGroup::add_worker] and may clone
    /// the supplied [`WorkerGroups`] to support later asynchronous group creation. The builder
    /// counts those reservations and waits for each worker to complete its initialization
    /// callback after this method returns. If the policy cannot run a reserved worker, dropping
    /// it cancels that target.
    fn bind(self, context: BindContext<'_>) -> Result<impl BoundPolicy, BindError>
    where
        Self: Sized;
}

/// Selects task destinations after a [`WorkerPolicy`] binds to one pool.
///
/// This type is shared by schedulers, so `Send + Sync` make concurrent selection part of its
/// contract. The `Pool` public API preserves its unwind-safety contract across shutdown and
/// submission boundaries, so bound policies are also `UnwindSafe + RefUnwindSafe`. `Debug`
/// exposes the active policy in pool diagnostics. Selection should keep the established routing
/// path lock-free and avoid cloning [`WorkerGroup`][crate::worker_policy::WorkerGroup] handles;
/// group creation and worker startup may clone handles because they occur off that path.
pub trait BoundPolicy: Debug + Send + Sync + UnwindSafe + RefUnwindSafe + 'static {
    /// Selects the group for one submitted task.
    ///
    /// The returned ID is pool-specific and copied into the scheduler's enqueue path
    /// without cloning a worker group. The first selection may create groups or workers;
    /// established selection should remain on the ordinary task fast path. Policies that
    ///     retain a [`WorkerGroup`][crate::worker_policy::WorkerGroup] can use
    ///     [`WorkerGroup::queue_len`][crate::worker_policy::WorkerGroup::queue_len] to observe
    ///     existing backlog, but selection runs before the incoming task is added to that queue.
    fn select_group(&self, groups: &WorkerGroups) -> WorkerGroupId;
}

/// Erases an unbound policy while preserving a by-value binding operation.
pub(crate) trait WorkerPolicyBinding: Debug {
    fn bind(self: Box<Self>, context: BindContext<'_>) -> Result<Box<dyn BoundPolicy>, BindError>;
}

impl<T> WorkerPolicyBinding for T
where
    T: WorkerPolicy,
{
    fn bind(self: Box<Self>, context: BindContext<'_>) -> Result<Box<dyn BoundPolicy>, BindError> {
        let bound = (*self).bind(context)?;
        Ok(Box::new(bound))
    }
}
