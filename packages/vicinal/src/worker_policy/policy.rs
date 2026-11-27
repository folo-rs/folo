use std::any::type_name_of_val;
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
/// `Debug` lets callers format policy values directly. Builder diagnostics report only whether
/// a policy was explicitly configured, without calling its formatter. The unbound configuration is
/// consumed during construction, so it needs none of the thread-safety contracts required by
/// the value returned from [`bind`][Self::bind]. The configuration must be `'static` so binding
/// can be called when the builder is consumed.
///
/// # Example
///
/// ```
/// # use std::error::Error;
/// use std::thread::Builder;
///
/// use vicinal::Pool;
/// use vicinal::worker_policy::{
///     BindContext, BindError, BoundPolicy, WorkerGroupId, WorkerGroups, WorkerPolicy,
/// };
/// # use futures::executor::block_on;
///
/// #[derive(Debug)]
/// struct OneGroupPolicy;
///
/// impl WorkerPolicy for OneGroupPolicy {
///     fn bind(self, context: BindContext<'_>) -> Result<impl BoundPolicy, BindError> {
///         let group = context.groups().add();
///         let worker = group.add_worker();
///         _ = Builder::new()
///             .spawn(move || worker.run())
///             .map_err(BindError::new)?;
///         Ok(BoundOneGroupPolicy { group: group.id() })
///     }
/// }
///
/// #[derive(Debug)]
/// struct BoundOneGroupPolicy {
///     group: WorkerGroupId,
/// }
///
/// impl BoundPolicy for BoundOneGroupPolicy {
///     fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
///         self.group
///     }
/// }
///
/// # block_on(async {
/// let pool = Pool::builder().worker_policy(OneGroupPolicy).build()?;
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
    /// waits for reserved workers to finish initialization before returning a pool. Dropping
    /// an unrun worker cancels its reservation. The policy must resolve each reservation
    /// without waiting for pool construction to return.
    fn bind(self, context: BindContext<'_>) -> Result<impl BoundPolicy, BindError>
    where
        Self: Sized;
}

/// Selects task destinations after a [`WorkerPolicy`] binds to one pool.
///
/// This type is shared by schedulers, so `Send + Sync` make concurrent selection part of its
/// contract. The `Pool` public API preserves its unwind-safety contract across shutdown and
/// submission boundaries, so bound policies are also `UnwindSafe + RefUnwindSafe`. `Debug`
/// lets callers format policy values directly. Pool and scheduler diagnostics identify the bound
/// policy type without calling its formatter. Selection should keep the established routing
/// path lock-free and avoid cloning [`WorkerGroup`][crate::worker_policy::WorkerGroup] handles;
/// group creation and worker startup may clone handles because they occur off that path.
pub trait BoundPolicy: Debug + Send + Sync + UnwindSafe + RefUnwindSafe + 'static {
    /// Selects the group for one submitted task.
    ///
    /// Return an ID belonging to the supplied pool. Selection runs before admission and does
    /// not guarantee that the scheduler will enqueue the task.
    ///
    /// Selection may create groups or workers, but keep established routing fast. Policies
    /// that retain a [`WorkerGroup`][crate::worker_policy::WorkerGroup] can use
    /// [`WorkerGroup::queue_len`][crate::worker_policy::WorkerGroup::queue_len] to observe
    /// existing backlog without counting the incoming task.
    fn select_group(&self, groups: &WorkerGroups) -> WorkerGroupId;
}

/// Erases an unbound policy while preserving a by-value binding operation.
pub(crate) trait WorkerPolicyBinding: Debug {
    fn bind(
        self: Box<Self>,
        context: BindContext<'_>,
    ) -> Result<(Box<dyn BoundPolicy>, &'static str), BindError>;
}

impl<T> WorkerPolicyBinding for T
where
    T: WorkerPolicy,
{
    fn bind(
        self: Box<Self>,
        context: BindContext<'_>,
    ) -> Result<(Box<dyn BoundPolicy>, &'static str), BindError> {
        let bound = (*self).bind(context)?;
        // Capture the concrete type before erasure so diagnostics need not call policy code.
        let policy_type = type_name_of_val(&bound);
        Ok((Box::new(bound), policy_type))
    }
}
