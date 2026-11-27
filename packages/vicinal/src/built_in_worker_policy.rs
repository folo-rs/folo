//! Private implementation shared by the public built-in worker policies.

use std::any::type_name;
use std::collections::BTreeSet;
use std::fmt::{Debug, Formatter, Result as FmtResult};
use std::iter::repeat_with;
use std::num::NonZero;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use many_cpus::{MemoryRegionId, Processor, ProcessorId, ProcessorSet, SystemHardware};

use crate::worker_policy::{
    BindContext, BindError, BoundPolicy, WorkerGroup, WorkerGroupId, WorkerGroups, WorkerPolicy,
};
use crate::{EagerProcessorHardwareMismatch, GroupId, NEVER_POISONED, SubmissionTarget};
/// Owns the common routing and worker-startup implementation for all built-in policies.
///
/// The public policy facades remain small and stable while this type selects the topology
/// strategy and stores only its pool-specific bound form. That lets the crate add or refine
/// built-in routing choices without making the shared implementation part of the public API.
#[derive(Debug)]
pub(crate) struct BuiltInWorkerPolicy {
    /// Selects processor-local, regional or system-wide routing semantics.
    kind: PolicyKind,
    /// Fixed concurrency assigned to each destination selected by this policy kind.
    count: NonZero<u32>,
}

impl BuiltInWorkerPolicy {
    #[cfg(test)]
    /// Returns a destination matching the current processor for white-box startup fixtures.
    ///
    /// Production selection routes through the bound policy; this helper lets tests arrange the
    /// same internal destination without duplicating the sampled topology mapping.
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
    /// Workers initially run on the region's eager processors or first submitting processor.
    /// Other submitting processors in that region become permitted without increasing workers.
    #[must_use]
    pub(crate) fn per_memory_region(count: NonZero<u32>) -> Self {
        Self::new(PolicyKind::MemoryRegion, count)
    }

    /// Uses one fixed worker count across the system's submitting processors.
    ///
    /// Workers initially run on eager processors or the first submitting processor. They become
    /// permitted on later submitting processors across the system without adding workers.
    /// The count is per pool rather than per processor or memory region.
    #[must_use]
    pub(crate) fn per_system(count: NonZero<u32>) -> Self {
        Self::new(PolicyKind::System, count)
    }

    pub(crate) fn worker_count(&self) -> NonZero<u32> {
        self.count
    }

    #[cfg(test)]
    pub(crate) fn kind(&self) -> PolicyKind {
        self.kind
    }

    fn new(kind: PolicyKind, count: NonZero<u32>) -> Self {
        Self { kind, count }
    }
}

impl Clone for BuiltInWorkerPolicy {
    fn clone(&self) -> Self {
        Self::new(self.kind, self.count)
    }
}

impl WorkerPolicy for BuiltInWorkerPolicy {
    fn bind(self, context: BindContext<'_>) -> Result<impl BoundPolicy, BindError> {
        Ok(self.bind_built_in(context))
    }
}

impl BuiltInWorkerPolicy {
    pub(crate) fn bind_built_in(self, context: BindContext<'_>) -> BoundBuiltInPolicy {
        BoundBuiltInPolicy::new(
            context.hardware(),
            context.eager(),
            context.groups(),
            self.kind,
            self.count,
        )
    }
}

impl BoundPolicy for BoundBuiltInPolicy {
    fn select_group(&self, groups: &WorkerGroups) -> WorkerGroupId {
        Self::select_group(self, groups, self.count())
    }
}

#[cfg(test)]
impl BoundPolicy for Arc<BoundBuiltInPolicy> {
    fn select_group(&self, groups: &WorkerGroups) -> WorkerGroupId {
        BoundBuiltInPolicy::select_group(self.as_ref(), groups, self.count())
    }
}

/// Verifies eager handles against both selected and backing hardware identities.
pub(crate) fn validate_eager_processors(
    hardware: &SystemHardware,
    processors: &ProcessorSet,
) -> Result<(), EagerProcessorHardwareMismatch> {
    let eligible: BTreeSet<_> = hardware.all_processors().into_iter().collect();
    let backing_processors = processors
        .to_builder()
        .take_all()
        .expect("ProcessorSet::to_builder retains the nonempty set's backing candidates");
    let belongs_to_pool_hardware = processors
        .iter()
        .chain(backing_processors.iter())
        .all(|processor| eligible.contains(processor));
    if !belongs_to_pool_hardware {
        return Err(EagerProcessorHardwareMismatch::new());
    }

    Ok(())
}

/// Selects the topology semantics used by one built-in policy facade.
///
/// Keeping this enum private lets the public facade types present independent choices while
/// sharing one implementation without exposing future variants as an exhaustive contract.
#[derive(Clone, Copy, Debug)]
pub(crate) enum PolicyKind {
    Processor,
    MemoryRegion,
    System,
}

/// Holds the pool-specific caches required for one bound built-in policy.
///
/// Processor routes use indexed slots, regional routes add compact ordered topology IDs, and
/// system routing uses one slot. Each variant also owns the readiness facts that let task
/// submission avoid the registry lock after a route has been published.
pub(crate) enum BoundBuiltInPolicy {
    Processor {
        hardware: SystemHardware,
        count: NonZero<u32>,
        groups: Box<[OnceLock<WorkerGroupId>]>,
    },
    MemoryRegion {
        hardware: SystemHardware,
        count: NonZero<u32>,
        ids: Box<[MemoryRegionId]>,
        groups: Box<[OnceLock<WorkerGroupId>]>,
        members: Box<[(ProcessorId, AtomicBool)]>,
    },
    System {
        hardware: SystemHardware,
        count: NonZero<u32>,
        group: OnceLock<WorkerGroupId>,
        members: Box<[(ProcessorId, AtomicBool)]>,
    },
}

impl BoundBuiltInPolicy {
    fn count(&self) -> NonZero<u32> {
        match self {
            Self::Processor { count, .. }
            | Self::MemoryRegion { count, .. }
            | Self::System { count, .. } => *count,
        }
    }

    fn new(
        hardware: &SystemHardware,
        eager: Option<&ProcessorSet>,
        groups: &WorkerGroups,
        kind: PolicyKind,
        count: NonZero<u32>,
    ) -> Self {
        let bound = match kind {
            PolicyKind::Processor => Self::Processor {
                hardware: hardware.clone(),
                count,
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
                    count,
                    groups: repeat_with(OnceLock::new).take(ids.len()).collect(),
                    ids,
                    members: member_processors(hardware),
                }
            }
            PolicyKind::System => Self::System {
                hardware: hardware.clone(),
                count,
                group: OnceLock::new(),
                members: member_processors(hardware),
            },
        };

        if let Some(processors) = eager {
            bound.bind_eager(groups, kind, count, processors.clone());
        }

        bound
    }

    /// Seeds all eager destinations before starting and waiting for their workers.
    ///
    /// Shared masks, revisions and membership readiness are complete before worker startup.
    /// Each worker applies its startup pin and records the published revision before it joins
    /// participation, so construction cannot expose an expandable route before those facts
    /// agree. Ref: docs/implementation.md, "Affinity and failure coordination".
    fn bind_eager(
        &self,
        registry: &WorkerGroups,
        kind: PolicyKind,
        count: NonZero<u32>,
        processors: ProcessorSet,
    ) {
        let mut groups = Vec::new();
        for_each_eager_group(kind, processors, |destination, affinity| {
            let group = registry.add_for(destination);
            self.publish_initial(&group, destination, &affinity);
            self.install_eager_group(destination, group.id());
            groups.push((destination, group, affinity));
        });
        for (destination, group, affinity) in groups {
            group.start_eager_workers(destination, &affinity, count);
        }
    }

    fn select_group(&self, registry: &WorkerGroups, count: NonZero<u32>) -> WorkerGroupId {
        match self {
            Self::Processor {
                hardware, groups, ..
            } => {
                let processor = hardware.current_processor_id();
                let id = groups
                    .get(processor as usize)
                    .expect("current_processor_id returns an ID present in this hardware's slots")
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
                ..
            } => {
                let (region_id, processor_id) = hardware.with_current_processor(|processor| {
                    (processor.memory_region_id(), processor.id())
                });
                let index = ids
                    .binary_search(&region_id)
                    .expect("ids was built from every region in this hardware topology");
                let target = SubmissionTarget {
                    group: GroupId::MemoryRegion(region_id),
                    processor: processor_id,
                };
                let id = groups
                    .get(index)
                    .expect(
                        "binary_search returned an index from groups built at the same ids length",
                    )
                    .get_or_init(|| {
                        let id = self.create_group(registry, count, target.group, target.processor);
                        // The first mask exists before the route can be observed. Publish its
                        // submitter before OnceLock exposes the ID, otherwise another first
                        // submitter could rebuild a mask without this processor.
                        membership(members, target.processor).store(true, Ordering::Release);
                        id
                    });
                #[cfg(test)]
                if let Some(before_selection) =
                    registry.resolve(*id).before_membership_selection.get()
                {
                    before_selection();
                }
                self.expand_if_needed(registry, members, target, *id);
                *id
            }
            Self::System {
                hardware,
                group,
                members,
                ..
            } => {
                let target = SubmissionTarget {
                    group: GroupId::System,
                    processor: hardware.current_processor_id(),
                };
                let id = group.get_or_init(|| {
                    let id = self.create_group(registry, count, target.group, target.processor);
                    // OnceLock exposes the route only after the initial member participates in
                    // future mask reconstruction, preventing concurrent first-use shrinkage.
                    membership(members, target.processor).store(true, Ordering::Release);
                    id
                });
                #[cfg(test)]
                if let Some(before_selection) =
                    registry.resolve(*id).before_membership_selection.get()
                {
                    before_selection();
                }
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
        let affinity = Arc::new(
            self.hardware()
                .all_processors()
                .to_builder()
                .filter(|candidate| candidate.id() == processor && destination.contains(candidate))
                .take_all()
                .unwrap_or_else(|| {
                    panic!(
                        "topology-derived destination {destination:?} contained no sampled processor"
                    )
                }),
        );
        let group = registry.add_for(destination);

        if !matches!(destination, GroupId::Processor(_)) {
            // Regional and system routes may widen, so their workers need a shared snapshot
            // and revision. Processor routes receive a fixed singleton startup pin and never
            // use this shared-publication path. Ref: docs/implementation.md, "Affinity and
            // failure coordination".
            *group.state().affinity.write().expect(NEVER_POISONED) = Some(Arc::clone(&affinity));
            group
                .state()
                .affinity_revision
                .fetch_add(1, Ordering::Release);
        }

        // The worker applies this pin and reads its initial revision before publishing
        // participation. Lazy startup waits for that participation before exposing the route,
        // so the worker-local revision cannot refer to a wider mask than the pin it applied.
        group.start_workers(destination, &affinity, count);
        group.id()
    }

    /// Expands a shared route and publishes processor readiness.
    ///
    /// A membership `Release`/`Acquire` handoff certifies that the matching mask and revision
    /// are available to the lock-free submitter. The separately acquired revision lets a
    /// worker observe the snapshot; only applying that snapshot pins the worker.
    /// Ref: docs/implementation.md, "Affinity and failure coordination".
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
        // This is the lock-free fast path. The matching Release store below happens only after
        // the mask and revision exist, so a ready observation requires no topology or group lock.
        if member.load(Ordering::Acquire) {
            return;
        }

        let state = registry.resolve(id);
        #[cfg(test)]
        if let Some(before_lock) = state.before_membership_lock.get() {
            before_lock();
        }
        let mut current = state.affinity.write().expect(NEVER_POISONED);
        // The writer-guarded recheck and reconstruction loads below observe Release-published
        // flags from other slow-path publishers. They close those races but do not replace the
        // lock-free readiness handoff above.
        if member.load(Ordering::Acquire) {
            return;
        }
        if current.as_ref().is_some_and(|current| {
            current
                .iter()
                .any(|processor| processor.id() == target.processor)
        }) {
            // Lazy group creation installed its singleton mask and revision but intentionally
            // left readiness unpublished. Only readiness remains; no revision or wakeup is
            // needed until a later submission widens the mask.
            member.store(true, Ordering::Release);
            return;
        }

        // The publication lock serializes mask construction with membership observations. The
        // current processor is included explicitly before readiness reaches the lock-free path.
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
            .expect("the filter explicitly includes the sampled processor for this destination");
        *current = Some(Arc::new(affinity));
        // Workers acquire this revision to find the corresponding immutable snapshot. Seeing a
        // revision does not itself mean that the worker has applied the snapshot.
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
            // Processor-local workers keep the fixed singleton pin applied by startup. Their
            // route cannot widen, so it needs neither a shared revision nor membership readiness.
            return;
        }

        *group.state().affinity.write().expect(NEVER_POISONED) = Some(Arc::new(affinity.clone()));
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
        self.route_slot(destination)
            .set(group)
            .expect("for_each_eager_group emits each destination exactly once");
    }

    /// Resolves the shared route layout for eager and fixture-only installation.
    fn route_slot(&self, destination: GroupId) -> &OnceLock<WorkerGroupId> {
        match (self, destination) {
            (Self::Processor { groups, .. }, GroupId::Processor(id)) => groups
                .get(id as usize)
                .expect("processor slots cover max_processor_count from this same hardware"),
            (Self::MemoryRegion { ids, groups, .. }, GroupId::MemoryRegion(id)) => groups
                .get(
                    ids.binary_search(&id)
                        .expect("regions are collected from this hardware before slots are built"),
                )
                .expect("binary search found an index from the matching ids and groups arrays"),
            (Self::System { group, .. }, GroupId::System) => group,
            _ => {
                panic!("select_group constructed a destination that does not match its policy kind")
            }
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
            Self::Processor { .. } => {
                panic!("processor-local selection cannot request shared membership storage")
            }
        }
    }

    #[cfg(test)]
    /// Installs a controlled group after a test has arranged its exact destination.
    ///
    /// Production code creates and publishes routes through `bind()` and `select_group()`.
    /// Startup and failure fixtures use this test-only path to reuse real route slots without
    /// depending on whichever processor the test runner happens to execute on.
    pub(crate) fn install_test_group(
        &self,
        group: &WorkerGroup,
        kind: PolicyKind,
        affinity: &ProcessorSet,
    ) {
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

        let slot = self.route_slot(destination);
        if let Some(existing) = slot.get() {
            assert_eq!(*existing, group.id());
        } else {
            slot.set(group.id())
                .expect("the test route is initially empty");
        }
    }
}

impl Debug for BoundBuiltInPolicy {
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
        .expect("member_processors copied every processor in this hardware topology");
    &members
        .get(index)
        .expect("binary_search returned an index within the sorted membership slice")
        .1
}

/// Visits unique eager destinations while selecting each destination's affinity.
///
/// Registration consumes each result immediately so startup retains only the completed group
/// tuples instead of first allocating a destination buffer.
fn for_each_eager_group(
    policy: PolicyKind,
    processors: ProcessorSet,
    mut visit: impl FnMut(GroupId, ProcessorSet),
) {
    match policy {
        PolicyKind::Processor => {
            let mut destinations = BTreeSet::new();
            for affinity in processors.decompose() {
                let id = affinity.processors().first().id();
                if destinations.insert(id) {
                    visit(GroupId::Processor(id), affinity);
                }
            }
        }
        PolicyKind::MemoryRegion => {
            let regions = processors
                .iter()
                .map(Processor::memory_region_id)
                .collect::<BTreeSet<_>>();
            for region in regions {
                let affinity = processors
                    .filter(|processor| processor.memory_region_id() == region)
                    .expect("regions are decomposed directly from the selected processor set");
                visit(GroupId::MemoryRegion(region), affinity);
            }
        }
        PolicyKind::System => visit(GroupId::System, processors),
    }
}
