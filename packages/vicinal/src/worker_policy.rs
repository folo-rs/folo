use std::any::type_name;
use std::collections::BTreeSet;
use std::fmt::{Debug, Formatter, Result as FmtResult};
use std::iter::repeat_with;
use std::num::NonZero;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use many_cpus::{MemoryRegionId, Processor, ProcessorId, ProcessorSet, SystemHardware};

use crate::{GroupId, NEVER_POISONED, SubmissionTarget, WorkerGroup, WorkerGroupId, WorkerGroups};

/// Controls how submitted tasks are assigned to worker groups.
///
/// Policies are prepared once while the pool is built and then shared by all schedulers.
/// [`select_group`](Self::select_group) should keep established routing lock-free and avoid
/// cloning [`WorkerGroup`] handles. Group creation and worker startup may clone handles,
/// because those operations occur off the ordinary task-routing path.
///
/// A policy-created thread applies its intended affinity and then calls
/// [`WorkerGroup::execute_worker`], which blocks until the worker leaves the pool.
///
/// # Example
///
/// ```
/// # use std::error::Error;
/// use std::sync::OnceLock;
/// use std::thread;
///
/// use vicinal::{Pool, WorkerGroupId, WorkerGroups, WorkerPolicy};
/// # use futures::executor::block_on;
///
/// #[derive(Default)]
/// struct OneGroupPolicy {
///     group: OnceLock<WorkerGroupId>,
/// }
///
/// impl WorkerPolicy for OneGroupPolicy {
///     fn select_group(&self, groups: &WorkerGroups) -> WorkerGroupId {
///         *self.group.get_or_init(|| {
///             let group = groups.add();
///             thread::spawn({
///                 let group = group.clone();
///                 move || group.execute_worker()
///             });
///             group.id()
///         })
///     }
/// }
///
/// # block_on(async {
/// let pool = Pool::builder()
///     .worker_policy(OneGroupPolicy::default())
///     .build();
/// assert_eq!(pool.scheduler().spawn(|| 42).await?, 42);
/// # Ok::<(), Box<dyn Error>>(())
/// # }).unwrap();
/// ```
pub trait WorkerPolicy: Send + Sync + UnwindSafe + RefUnwindSafe + 'static {
    /// Performs one-time initialization using the pool's final configuration.
    ///
    /// The default implementation does nothing. If [`WorkerPolicyContext::eager`] is
    /// present, a policy that honors eager construction creates and initializes the
    /// corresponding workers before returning. The method may clone the supplied
    /// [`WorkerGroups`] to support later asynchronous group creation. After spawning
    /// its eager workers, a policy can call [`WorkerGroup::wait_until_initialized`] for
    /// each group to wait for their initialization callbacks.
    fn prepare(&mut self, _context: WorkerPolicyContext<'_>) {}

    /// Selects the group for one submitted task.
    ///
    /// The returned ID is pool-specific and copied into the scheduler's enqueue path
    /// without cloning a worker group. The first selection may create groups or workers;
    /// established selection should remain on the ordinary task fast path. Policies that
    /// retain a [`WorkerGroup`] can use [`WorkerGroup::queue_len`] to observe existing
    /// backlog, but selection runs before the incoming task is added to that queue.
    fn select_group(&self, groups: &WorkerGroups) -> WorkerGroupId;

    #[cfg(test)]
    #[doc(hidden)]
    /// Returns a test snapshot of built-in-policy processor membership.
    fn membership_snapshot(&self) -> Vec<(ProcessorId, bool)> {
        Vec::new()
    }

    #[cfg(test)]
    #[doc(hidden)]
    /// Installs an internally constructed group into a built-in policy's route.
    fn install_test_group(
        &self,
        _groups: &WorkerGroups,
        _group: &WorkerGroup,
        _affinity: &ProcessorSet,
    ) {
    }
}

/// Final pool configuration made available while a policy is prepared.
#[derive(Clone, Copy)]
pub struct WorkerPolicyContext<'a> {
    hardware: &'a SystemHardware,
    eager: Option<&'a ProcessorSet>,
    groups: &'a WorkerGroups,
    pool_name: &'a str,
}

impl Debug for WorkerPolicyContext<'_> {
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

impl WorkerPolicyContext<'_> {
    pub(crate) fn new<'a>(
        hardware: &'a SystemHardware,
        eager: Option<&'a ProcessorSet>,
        groups: &'a WorkerGroups,
        pool_name: &'a str,
    ) -> WorkerPolicyContext<'a> {
        WorkerPolicyContext {
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

/// Shares the implementation used by the public built-in policy facades.
#[derive(Debug)]
pub(crate) struct BuiltInWorkerPolicy {
    kind: PolicyKind,
    count: NonZero<u32>,
    prepared: Option<PreparedPolicy>,
}

impl BuiltInWorkerPolicy {
    #[cfg(test)]
    pub(crate) fn destination(&self, hardware: &SystemHardware) -> SubmissionTarget {
        match self.kind {
            PolicyKind::Processor => {
                let processor = hardware.current_processor_id();
                SubmissionTarget {
                    group: GroupId::Processor(processor),
                    processor,
                }
            }
            PolicyKind::MemoryRegion => {
                hardware.with_current_processor(|processor| SubmissionTarget {
                    group: GroupId::MemoryRegion(processor.memory_region_id()),
                    processor: processor.id(),
                })
            }
            PolicyKind::System => SubmissionTarget {
                group: GroupId::System,
                processor: hardware.current_processor_id(),
            },
        }
    }

    /// Uses a fixed number of workers pinned to each active processor.
    ///
    /// Each task is routed to the logical processor sampled once during submission.
    #[must_use]
    pub(crate) fn per_processor(count: NonZero<u32>) -> Self {
        Self::new(PolicyKind::Processor, count)
    }

    /// Uses a fixed worker count per submitting memory region.
    ///
    /// A region's worker mask starts with its eager processors or first submitter and
    /// expands cumulatively when processors in that region reach policy selection.
    /// Adding a processor changes affinity without adding workers.
    #[must_use]
    pub(crate) fn per_memory_region(count: NonZero<u32>) -> Self {
        Self::new(PolicyKind::MemoryRegion, count)
    }

    /// Uses one fixed worker count across the system's submitting processors.
    ///
    /// The worker mask starts with eager processors or the first submitter and expands
    /// cumulatively when processors reach selection across memory-region boundaries. The
    /// count is per pool rather than per processor or memory region.
    #[must_use]
    pub(crate) fn per_system(count: NonZero<u32>) -> Self {
        Self::new(PolicyKind::System, count)
    }

    pub(crate) fn worker_count(&self) -> NonZero<u32> {
        self.count
    }

    fn new(kind: PolicyKind, count: NonZero<u32>) -> Self {
        Self {
            kind,
            count,
            prepared: None,
        }
    }
}

impl Clone for BuiltInWorkerPolicy {
    fn clone(&self) -> Self {
        Self::new(self.kind, self.count)
    }
}

impl WorkerPolicy for BuiltInWorkerPolicy {
    fn prepare(&mut self, context: WorkerPolicyContext<'_>) {
        self.prepared = Some(PreparedPolicy::new(
            context.hardware(),
            context.eager(),
            context.groups(),
            self.kind,
            self.count,
        ));
    }

    fn select_group(&self, groups: &WorkerGroups) -> WorkerGroupId {
        self.prepared
            .as_ref()
            .expect("the builder prepares a policy before scheduling")
            .select_group(groups, self.count)
    }

    #[cfg(test)]
    fn membership_snapshot(&self) -> Vec<(ProcessorId, bool)> {
        self.prepared
            .as_ref()
            .expect("the builder prepares a policy before inspection")
            .membership_snapshot()
    }

    #[cfg(test)]
    fn install_test_group(
        &self,
        _groups: &WorkerGroups,
        group: &WorkerGroup,
        affinity: &ProcessorSet,
    ) {
        self.prepared
            .as_ref()
            .expect("the builder prepares a policy before installation")
            .install_test_group(group, self.kind, affinity);
    }
}

/// Verifies eager handles against both selected and backing hardware identities.
pub(crate) fn validate_eager_processors(hardware: &SystemHardware, processors: &ProcessorSet) {
    let eligible: BTreeSet<_> = hardware.all_processors().into_iter().collect();
    let backing_processors = processors
        .to_builder()
        .take_all()
        .expect("a nonempty selection retains candidates on its backing hardware");
    assert!(
        processors
            .iter()
            .chain(backing_processors.iter())
            .all(|processor| eligible.contains(processor))
    );
}

/// Keeps built-in policy variants private so future choices need not become exhaustive APIs.
#[derive(Clone, Copy, Debug)]
pub(crate) enum PolicyKind {
    Processor,
    MemoryRegion,
    System,
}

/// Stores pool-specific routing slots, hardware and membership publication state.
enum PreparedPolicy {
    Processor {
        hardware: SystemHardware,
        groups: Box<[OnceLock<WorkerGroupId>]>,
    },
    MemoryRegion {
        hardware: SystemHardware,
        ids: Box<[MemoryRegionId]>,
        groups: Box<[OnceLock<WorkerGroupId>]>,
        members: Box<[(ProcessorId, AtomicBool)]>,
    },
    System {
        hardware: SystemHardware,
        group: OnceLock<WorkerGroupId>,
        members: Box<[(ProcessorId, AtomicBool)]>,
    },
}

impl PreparedPolicy {
    fn new(
        hardware: &SystemHardware,
        eager: Option<&ProcessorSet>,
        groups: &WorkerGroups,
        kind: PolicyKind,
        count: NonZero<u32>,
    ) -> Self {
        let prepared = match kind {
            PolicyKind::Processor => Self::Processor {
                hardware: hardware.clone(),
                groups: repeat_with(OnceLock::new)
                    .take(hardware.max_processor_count())
                    .collect(),
            },
            PolicyKind::MemoryRegion => {
                let ids = hardware
                    .all_processors()
                    .iter()
                    .map(Processor::memory_region_id)
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect::<Box<[_]>>();
                Self::MemoryRegion {
                    hardware: hardware.clone(),
                    groups: repeat_with(OnceLock::new).take(ids.len()).collect(),
                    ids,
                    members: member_processors(hardware),
                }
            }
            PolicyKind::System => Self::System {
                hardware: hardware.clone(),
                group: OnceLock::new(),
                members: member_processors(hardware),
            },
        };

        if let Some(processors) = eager {
            prepared.prepare_eager(groups, kind, count, processors.clone());
        }

        prepared
    }

    /// Seeds all eager destinations before starting and waiting for their workers.
    fn prepare_eager(
        &self,
        registry: &WorkerGroups,
        kind: PolicyKind,
        count: NonZero<u32>,
        processors: ProcessorSet,
    ) {
        let groups = eager_groups(kind, processors)
            .into_iter()
            .map(|(destination, affinity)| {
                let group = registry.add_for(destination);
                self.publish_initial(&group, destination, &affinity);
                self.install_eager_group(destination, group.id());
                (destination, group, affinity)
            })
            .collect::<Vec<_>>();
        let workers = groups
            .iter()
            .map(|(destination, group, affinity)| {
                (
                    group.clone(),
                    group.start_eager_workers(*destination, affinity, count),
                )
            })
            .collect::<Vec<_>>();

        for (group, expected) in workers {
            group.wait_for_eager_workers(expected);
        }
    }

    fn select_group(&self, registry: &WorkerGroups, count: NonZero<u32>) -> WorkerGroupId {
        match self {
            Self::Processor { hardware, groups } => {
                let processor = hardware.current_processor_id();
                let id = groups
                    .get(processor as usize)
                    .expect("a submitting processor belongs to its hardware index space")
                    .get_or_init(|| {
                        self.create_group(registry, count, GroupId::Processor(processor), processor)
                    });
                *id
            }
            Self::MemoryRegion {
                hardware,
                ids,
                groups,
                members,
            } => {
                let target = hardware.with_current_processor(|processor| SubmissionTarget {
                    group: GroupId::MemoryRegion(processor.memory_region_id()),
                    processor: processor.id(),
                });
                let index = ids
                    .binary_search(&match target.group {
                        GroupId::MemoryRegion(id) => id,
                        _ => unreachable!("memory-region policy selects a region"),
                    })
                    .expect("a submitting region contains an eligible processor");
                let id = groups
                    .get(index)
                    .expect("binary search identifies an existing region slot")
                    .get_or_init(|| {
                        self.create_group(registry, count, target.group, target.processor)
                    });
                self.expand_if_needed(registry, members, target, *id);
                *id
            }
            Self::System {
                hardware,
                group,
                members,
            } => {
                let target = SubmissionTarget {
                    group: GroupId::System,
                    processor: hardware.current_processor_id(),
                };
                let id = group.get_or_init(|| {
                    self.create_group(registry, count, target.group, target.processor)
                });
                self.expand_if_needed(registry, members, target, *id);
                *id
            }
        }
    }

    /// Creates a lazily selected destination and starts its full worker count.
    fn create_group(
        &self,
        registry: &WorkerGroups,
        count: NonZero<u32>,
        destination: GroupId,
        processor: ProcessorId,
    ) -> WorkerGroupId {
        let affinity = self
            .hardware()
            .all_processors()
            .to_builder()
            .filter(|candidate| candidate.id() == processor && destination.contains(candidate))
            .take_all()
            .unwrap_or_else(|| panic!("no eligible processors for worker group {destination:?}"));
        let group = registry.add_for(destination);

        if !matches!(destination, GroupId::Processor(_)) {
            *group.state().affinity.write().expect(NEVER_POISONED) = Some(affinity.clone());
            group
                .state()
                .affinity_revision
                .fetch_add(1, Ordering::Release);
        }

        group.start_workers(destination, &affinity, count);
        group.id()
    }

    // Omitting expansion can leave tests waiting on hooks that no producer will invoke.
    #[cfg_attr(test, mutants::skip)]
    fn expand_if_needed(
        &self,
        registry: &WorkerGroups,
        members: &[(ProcessorId, AtomicBool)],
        target: SubmissionTarget,
        id: WorkerGroupId,
    ) {
        let member = membership(members, target.processor);
        if member.load(Ordering::Acquire) {
            return;
        }

        let state = registry.resolve(id);
        #[cfg(test)]
        if let Some(before_lock) = state.before_membership_lock.get() {
            before_lock();
        }
        let mut current = state.affinity.write().expect(NEVER_POISONED);
        if member.load(Ordering::Acquire) {
            return;
        }
        if current.as_ref().is_some_and(|current| {
            current
                .iter()
                .any(|processor| processor.id() == target.processor)
        }) {
            member.store(true, Ordering::Release);
            return;
        }

        // The publication lock serializes mask/revision construction with membership readiness.
        // The current processor is included explicitly before its flag is released-published.
        let affinity = self
            .hardware()
            .all_processors()
            .to_builder()
            .filter(|processor| {
                target.group.contains(processor)
                    && (processor.id() == target.processor
                        || membership(members, processor.id()).load(Ordering::Acquire))
            })
            .take_all()
            .expect("the sampled processor belongs to its selected destination");
        *current = Some(affinity);
        state.affinity_revision.fetch_add(1, Ordering::Release);
        #[cfg(test)]
        if let Some(before_publish) = state.before_membership_publish.get() {
            before_publish();
        }
        // This Release publication is the readiness bit for the lock-free Acquire fast path:
        // observing it certifies that the matching affinity mask and revision are present.
        member.store(true, Ordering::Release);
        drop(current);
        state.wake_event.notify(usize::MAX);
    }

    fn publish_initial(&self, group: &WorkerGroup, destination: GroupId, affinity: &ProcessorSet) {
        if matches!(destination, GroupId::Processor(_)) {
            return;
        }

        *group.state().affinity.write().expect(NEVER_POISONED) = Some(affinity.clone());
        group
            .state()
            .affinity_revision
            .fetch_add(1, Ordering::Release);
        for processor in affinity {
            // The mask and revision above are published before this readiness flag.
            membership(self.members(), processor.id()).store(true, Ordering::Release);
        }
    }

    fn install_eager_group(&self, destination: GroupId, group: WorkerGroupId) {
        match (self, destination) {
            (Self::Processor { groups, .. }, GroupId::Processor(id)) => groups
                .get(id as usize)
                .expect("an eager processor belongs to the hardware index space")
                .set(group)
                .expect("eager destinations are unique"),
            (Self::MemoryRegion { ids, groups, .. }, GroupId::MemoryRegion(id)) => groups
                .get(ids.binary_search(&id).expect("eager region is populated"))
                .expect("region search identifies a group")
                .set(group)
                .expect("eager destinations are unique"),
            (Self::System { group: slot, .. }, GroupId::System) => slot
                .set(group)
                .expect("one eager selection creates one system group"),
            _ => panic!("the eager destination must match the prepared policy"),
        }
    }

    fn hardware(&self) -> &SystemHardware {
        match self {
            Self::Processor { hardware, .. }
            | Self::MemoryRegion { hardware, .. }
            | Self::System { hardware, .. } => hardware,
        }
    }

    fn members(&self) -> &[(ProcessorId, AtomicBool)] {
        match self {
            Self::MemoryRegion { members, .. } | Self::System { members, .. } => members,
            Self::Processor { .. } => panic!("processor-local policy has no shared membership"),
        }
    }

    #[cfg(test)]
    fn membership_snapshot(&self) -> Vec<(ProcessorId, bool)> {
        match self {
            Self::MemoryRegion { members, .. } | Self::System { members, .. } => members
                .iter()
                .map(|(id, member)| (*id, member.load(Ordering::Acquire)))
                .collect(),
            Self::Processor { .. } => Vec::new(),
        }
    }

    #[cfg(test)]
    fn install_test_group(&self, group: &WorkerGroup, kind: PolicyKind, affinity: &ProcessorSet) {
        let first = affinity.processors().first();
        let destination = match kind {
            PolicyKind::Processor => GroupId::Processor(first.id()),
            PolicyKind::MemoryRegion => GroupId::MemoryRegion(first.memory_region_id()),
            PolicyKind::System => GroupId::System,
        };
        let missing_affinity = group
            .state()
            .affinity
            .read()
            .expect(NEVER_POISONED)
            .is_none();
        if missing_affinity {
            self.publish_initial(group, destination, affinity);
        }

        match (self, destination) {
            (Self::Processor { groups, .. }, GroupId::Processor(id)) => {
                let slot = groups
                    .get(id as usize)
                    .expect("a test processor belongs to the hardware index space");
                if let Some(existing) = slot.get() {
                    assert_eq!(*existing, group.id());
                } else {
                    slot.set(group.id())
                        .expect("the test route is initially empty");
                }
            }
            (Self::MemoryRegion { ids, groups, .. }, GroupId::MemoryRegion(id)) => {
                let slot = groups
                    .get(ids.binary_search(&id).expect("test region is populated"))
                    .expect("region search identifies a group");
                if let Some(existing) = slot.get() {
                    assert_eq!(*existing, group.id());
                } else {
                    slot.set(group.id())
                        .expect("the test route is initially empty");
                }
            }
            (Self::System { group: slot, .. }, GroupId::System) => {
                if let Some(existing) = slot.get() {
                    assert_eq!(*existing, group.id());
                } else {
                    slot.set(group.id())
                        .expect("the test route is initially empty");
                }
            }
            _ => panic!("the test destination must match the prepared policy"),
        }
    }
}

impl Debug for PreparedPolicy {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.write_str(type_name::<Self>())
    }
}

fn member_processors(hardware: &SystemHardware) -> Box<[(ProcessorId, AtomicBool)]> {
    let mut processors = hardware
        .all_processors()
        .iter()
        .map(|processor| (processor.id(), AtomicBool::new(false)))
        .collect::<Vec<_>>();
    processors.sort_unstable_by_key(|(id, _)| *id);
    processors.into_boxed_slice()
}

fn membership(members: &[(ProcessorId, AtomicBool)], processor: ProcessorId) -> &AtomicBool {
    let index = members
        .binary_search_by_key(&processor, |(id, _)| *id)
        .expect("a submitting processor belongs to the eligible topology");
    &members
        .get(index)
        .expect("binary search identifies an eligible processor")
        .1
}

fn eager_groups(policy: PolicyKind, processors: ProcessorSet) -> Vec<(GroupId, ProcessorSet)> {
    match policy {
        PolicyKind::Processor => {
            let mut destinations = BTreeSet::new();
            processors
                .decompose()
                .into_iter()
                .filter(|set| destinations.insert(set.processors().first().id()))
                .map(|set| (GroupId::Processor(set.processors().first().id()), set))
                .collect()
        }
        PolicyKind::MemoryRegion => processors
            .iter()
            .map(Processor::memory_region_id)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|region| {
                let affinity = processors
                    .filter(|processor| processor.memory_region_id() == region)
                    .expect("each selected region contains a selected processor");
                (GroupId::MemoryRegion(region), affinity)
            })
            .collect(),
        PolicyKind::System => vec![(GroupId::System, processors)],
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(
        WorkerPolicyContext<'static>: Copy,
        UnwindSafe,
        RefUnwindSafe
    );
}
