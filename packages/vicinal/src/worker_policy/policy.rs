use std::any::type_name_of_val;
use std::fmt::Debug;
use std::panic::{RefUnwindSafe, UnwindSafe};

use crate::worker_policy::{BindContext, BindError, WorkerGroupId, WorkerGroups};

/// Supplies the one-time configuration for a pool's custom worker policy.
///
/// This is the policy's construction-time value, not its submission-time state. Binding consumes
/// it and returns a separate [`BoundPolicy`] that schedulers share for task routing. Keeping those
/// phases separate allows an implementation to validate configuration and create initial groups
/// before a pool exists while requiring thread-safe state only from the bound value.
///
/// A policy reserves a worker with
/// [`WorkerGroup::add_worker`][crate::worker_policy::WorkerGroup::add_worker], moves the returned
/// [`Worker`][crate::worker_policy::Worker] into a thread, applies its intended affinity and calls
/// [`Worker::run`][crate::worker_policy::Worker::run]. That method blocks until the worker leaves
/// the pool.
/// `Debug` lets callers format configuration values directly. Builder diagnostics report only whether
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
    /// Binds this configuration to one pool and creates its routing policy.
    ///
    /// A policy can create initial groups and workers with
    /// [`WorkerGroup::add_worker`][crate::worker_policy::WorkerGroup::add_worker] and may clone
    /// the supplied [`WorkerGroups`] to support later asynchronous group creation. The builder
    /// waits for reserved workers to finish initialization before returning a pool. Dropping
    /// an unrun worker cancels its reservation. The policy must resolve each reservation
    /// without waiting for pool construction to return.
    fn bind(self, context: BindContext<'_>) -> Result<impl BoundPolicy, BindError>
    where
        Self: Sized;
}

/// Owns the state that selects task destinations for one completed pool.
///
/// Schedulers can call this value concurrently and retain it until the pool shuts down, which is
/// why `Send + Sync` and the unwind-safety traits are part of the contract. `Debug`
/// lets callers format policy values directly. Pool and scheduler diagnostics identify the bound
/// policy type without calling its formatter. The policy should cache
/// [`WorkerGroupId`][crate::worker_policy::WorkerGroupId] values for established routes; group
/// creation and worker startup are cold paths and may retain or clone
/// [`WorkerGroup`][crate::worker_policy::WorkerGroup] handles.
pub trait BoundPolicy: Debug + Send + Sync + UnwindSafe + RefUnwindSafe + 'static {
    /// Selects the destination for one submitted task that reaches routing.
    ///
    /// Return an ID created by the supplied [`WorkerGroups`]. The scheduler calls this before
    /// admitting the task, so selection alone does not reserve queue capacity or guarantee that
    /// the task is accepted.
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
