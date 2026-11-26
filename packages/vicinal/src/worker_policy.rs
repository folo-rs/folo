use std::any::type_name;
use std::error::Error;
use std::fmt::{Debug, Formatter, Result as FmtResult};
use std::panic::{RefUnwindSafe, UnwindSafe};

use many_cpus::{ProcessorSet, SystemHardware};
use ohno::OhnoCore;

use crate::{WorkerGroupId, WorkerGroups};
/// Defines one pool's task routing and policy-owned worker topology.
///
/// Policies are prepared once while the pool is built and then shared by all schedulers.
/// [`select_group`](Self::select_group) should keep established routing lock-free and avoid
/// cloning [`WorkerGroup`][crate::WorkerGroup] handles. Group creation and worker startup may
/// clone handles because those operations occur off the ordinary task-routing path.
///
/// A policy reserves a worker with
/// [`WorkerGroup::add_worker`][crate::WorkerGroup::add_worker], moves the returned
/// [`Worker`][crate::Worker] into a thread, applies its intended affinity and calls
/// [`Worker::run`][crate::Worker::run]. That method blocks until the worker leaves the pool.
/// Implementations expose [`Debug`] state so pool diagnostics can identify the active policy.
///
/// # Example
///
/// ```
/// # use std::error::Error;
/// use std::thread::spawn;
///
/// use vicinal::{Pool, PrepareContext, PrepareError, WorkerGroupId, WorkerGroups, WorkerPolicy};
/// # use futures::executor::block_on;
///
/// #[derive(Debug, Default)]
/// struct OneGroupPolicy {
///     group: Option<WorkerGroupId>,
/// }
///
/// impl WorkerPolicy for OneGroupPolicy {
///     fn prepare(&mut self, context: PrepareContext<'_>) -> Result<(), PrepareError> {
///         let group = context.groups().add();
///         spawn({
///             let worker = group.add_worker();
///             move || worker.run()
///         });
///         self.group = Some(group.id());
///         Ok(())
///     }
///
///     fn select_group(&self, _groups: &WorkerGroups) -> WorkerGroupId {
///         self.group.expect("the builder prepares the policy once")
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
pub trait WorkerPolicy: Debug + Send + Sync + UnwindSafe + RefUnwindSafe + 'static {
    /// Performs one-time initialization using the pool's final configuration.
    ///
    /// A policy creates preparation-time workers with
    /// [`WorkerGroup::add_worker`][crate::WorkerGroup::add_worker] and may clone
    /// the supplied [`WorkerGroups`] to support later asynchronous group creation. The builder
    /// counts those reservations and waits for each worker to complete its initialization
    /// callback after this method returns. If the policy cannot run a reserved worker, dropping
    /// it cancels that target.
    fn prepare(&mut self, context: PrepareContext<'_>) -> Result<(), PrepareError>;

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

/// A worker policy failed while preparing a pool.
///
/// The error retains the policy's original failure in its source chain.
#[derive(ohno::Error)]
#[no_constructors]
#[display("worker policy preparation failed")]
pub struct PrepareError {
    #[error]
    core: OhnoCore,
}

impl UnwindSafe for PrepareError {}
impl RefUnwindSafe for PrepareError {}

impl PrepareError {
    /// Creates a preparation error from a policy-specific failure.
    #[must_use]
    pub fn new(error: impl Into<Box<dyn Error + Send + Sync>>) -> Self {
        Self {
            core: OhnoCore::from(error),
        }
    }
}

/// Final pool configuration supplied to [`WorkerPolicy::prepare`].
#[derive(Clone, Copy)]
pub struct PrepareContext<'a> {
    hardware: &'a SystemHardware,
    eager: Option<&'a ProcessorSet>,
    groups: &'a WorkerGroups,
    pool_name: &'a str,
}

impl Debug for PrepareContext<'_> {
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

impl PrepareContext<'_> {
    pub(crate) fn new<'a>(
        hardware: &'a SystemHardware,
        eager: Option<&'a ProcessorSet>,
        groups: &'a WorkerGroups,
        pool_name: &'a str,
    ) -> PrepareContext<'a> {
        PrepareContext {
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
        PrepareContext<'static>: Copy,
        Send,
        Sync,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(PrepareError: Send, Sync, UnwindSafe, RefUnwindSafe);

    #[test]
    fn preparation_context_formats() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
        let eager = hardware.all_processors();
        let pool = Pool::builder().hardware(hardware.clone()).build().unwrap();
        let context =
            PrepareContext::new(&hardware, Some(&eager), &pool.inner.groups, "formatting");

        _ = format!("{context:?}");
    }
}
