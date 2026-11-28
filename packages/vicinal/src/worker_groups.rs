//! Dynamic registry and copyable routing identities for worker groups.

use std::any::type_name;
use std::fmt::{Debug, Formatter, Result as FmtResult};
#[cfg(test)]
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::ptr::NonNull;
#[cfg(test)]
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::vec::IntoIter as VecIntoIter;

use crate::worker_policy::WorkerGroup;
use crate::{GroupId, NEVER_POISONED, PoolRuntime, WorkerGroupState};

// The starting value is arbitrary: identities need uniqueness only, and a registry
// receives its owner at construction rather than using a numeric initialization sentinel.
static NEXT_REGISTRY_ID: AtomicU64 = AtomicU64::new(1);

/// Owns the worker-group destinations created for one pool.
///
/// This is the registry a bound policy receives during routing. It can create new groups during
/// binding or selection, and a retained clone can add groups later. Every group and
/// [`WorkerGroupId`] created here belongs to this one pool; IDs from another collection cannot be
/// used for routing. Cloning the collection shares the registry but does not start workers or
/// create groups.
#[derive(Clone)]
pub struct WorkerGroups {
    inner: Arc<WorkerGroupsInner>,
}

impl WorkerGroups {
    pub(crate) fn new(runtime: Arc<PoolRuntime>) -> Self {
        // Checked transition preserves the unique-owner premise at the raw-pointer boundary:
        // exhaustion panics before a retained ID can alias a newly created registry.
        let owner = NEXT_REGISTRY_ID
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |owner| {
                owner.checked_add(1)
            })
            .expect(
                "registry IDs wrap only after 2^64 allocations, beyond the finite process lifetime",
            );
        Self {
            inner: Arc::new(WorkerGroupsInner {
                owner,
                runtime,
                groups: Mutex::new(Vec::new()),
                #[cfg(test)]
                before_membership_selection: OnceLock::new(),
            }),
        }
    }

    /// Adds one task destination and returns its participation and observation handle.
    ///
    /// To include a worker in binding-time readiness, reserve it with
    /// [`WorkerGroup::add_worker`] during binding before spawning its thread. Move the returned
    /// worker into the thread. Use [`WorkerGroupId`] for task routing.
    ///
    /// # Panics
    ///
    /// Panics after the pool has permanently failed. A cleanly shut-down registry can
    /// still be used by a policy to construct and immediately abandon a submission.
    #[must_use]
    #[inline]
    pub fn add(&self) -> WorkerGroup {
        self.add_inner(None)
    }

    fn add_inner(&self, destination: Option<GroupId>) -> WorkerGroup {
        let state = Arc::new(WorkerGroupState::new(destination));
        let id = WorkerGroupId {
            owner: self.inner.owner,
            state: NonNull::from(Arc::as_ref(&state)),
        };
        let group = WorkerGroup::new(
            Arc::clone(&self.inner.runtime),
            GroupRegistration { id, state },
        );
        #[cfg(test)]
        if let Some(before_membership) = self.inner.before_membership_selection.get() {
            assert!(
                group
                    .state()
                    .before_membership_selection
                    .set(Arc::clone(before_membership))
                    .is_ok(),
                "a newly created group has no membership hook"
            );
        }

        let mut groups = self.inner.groups.lock().expect(NEVER_POISONED);
        groups.push(group.clone());

        group
    }

    pub(crate) fn add_for(&self, destination: GroupId) -> WorkerGroup {
        self.add_inner(Some(destination))
    }

    /// Returns the current number of task destinations in this registry.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.groups.lock().expect(NEVER_POISONED).len()
    }

    /// Returns whether this registry has no task destinations.
    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns a snapshot of the current group handles.
    ///
    /// The snapshot clones every stored group handle. Policies that choose a group for each
    /// task should cache [`WorkerGroupId`] values instead of taking snapshots repeatedly.
    #[must_use]
    #[inline]
    pub fn iter(&self) -> VecIntoIter<WorkerGroup> {
        self.snapshot().into_iter()
    }

    pub(crate) fn resolve(&self, group: WorkerGroupId) -> &WorkerGroupState {
        assert!(
            group.owner == self.inner.owner,
            "policy returned a worker group ID from another pool; return an ID from this registry"
        );

        // SAFETY: With the matching unique owner, this ID came from our add_inner. That
        // method takes a non-null, aligned pointer to the initialized Arc state, passes the
        // exact pair to the handle and retains its clone in this registry before exposing it.
        // The registry never removes a group, so its borrow keeps that allocation alive.
        // State is shared through Arc and modified only via interior synchronization; no
        // exclusive reference exists alongside the returned shared reference.
        unsafe { group.state.as_ref() }
    }

    pub(crate) fn snapshot(&self) -> Vec<WorkerGroup> {
        self.inner.groups.lock().expect(NEVER_POISONED).clone()
    }

    #[cfg(test)]
    pub(crate) fn set_before_membership_selection(
        &self,
        hook: impl Fn() + Send + Sync + UnwindSafe + RefUnwindSafe + 'static,
    ) {
        assert!(
            self.inner
                .before_membership_selection
                .set(Arc::new(hook))
                .is_ok(),
            "test membership hook is installed once"
        );
    }

    #[cfg(test)]
    pub(crate) fn initialized(&self) -> InitializedGroups<'_> {
        let ids = self
            .inner
            .groups
            .lock()
            .expect(NEVER_POISONED)
            .iter()
            .map(WorkerGroup::id)
            .collect::<Vec<_>>()
            .into_iter();
        InitializedGroups { groups: self, ids }
    }

    #[cfg(test)]
    pub(crate) fn get_or_init(&self, destination: GroupId) -> &WorkerGroupState {
        self.resolve(self.group_id(destination))
    }

    #[cfg(test)]
    pub(crate) fn group(&self, destination: GroupId) -> WorkerGroup {
        let id = self.group_id(destination);
        self.inner
            .groups
            .lock()
            .expect(NEVER_POISONED)
            .iter()
            .find(|group| group.id() == id)
            .expect("this ID came from the destination's group_id lookup in this registry")
            .clone()
    }

    #[cfg(test)]
    pub(crate) fn group_id(&self, destination: GroupId) -> WorkerGroupId {
        let existing = self
            .inner
            .groups
            .lock()
            .expect(NEVER_POISONED)
            .iter()
            .find(|group| group.state().destination == Some(destination))
            .map(WorkerGroup::id);

        existing.unwrap_or_else(|| self.add_for(destination).id())
    }
}

impl Debug for WorkerGroups {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct(type_name::<Self>())
            .field("len", &self.len())
            .finish_non_exhaustive()
    }
}

impl IntoIterator for &WorkerGroups {
    type Item = WorkerGroup;
    type IntoIter = VecIntoIter<WorkerGroup>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl IntoIterator for WorkerGroups {
    type Item = WorkerGroup;
    type IntoIter = VecIntoIter<WorkerGroup>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Opaque, copyable identity for a worker group in one pool.
///
/// IDs let a bound policy route tasks without cloning group handles on established paths. Obtain
/// the value from [`WorkerGroup::id`] or route construction through the [`WorkerGroups`] supplied
/// during policy binding and selection. Returning an ID from another pool violates the
/// [`BoundPolicy::select_group`][crate::worker_policy::BoundPolicy::select_group] contract.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct WorkerGroupId {
    owner: u64,
    /// Stable address used by the owning registry for lock-free, ref-count-free resolution.
    ///
    /// An index into the registry's `Vec<WorkerGroup>` would require locking the vector and
    /// cloning its `Arc` on every submission because references cannot outlive the guard.
    /// Each state is instead independently allocated and retained for the registry's lifetime.
    state: NonNull<WorkerGroupState>,
}

// SAFETY: Moving an ID copies only its owner and address; it does not dereference or own
// the pointee. Resolution checks the registry owner and borrows the registry-retained state.
unsafe impl Send for WorkerGroupId {}

// SAFETY: Sharing an ID does not access the pointee. Only the matching registry can resolve
// it, returning a shared reference to state it retains for the duration of that borrow.
unsafe impl Sync for WorkerGroupId {}

impl Debug for WorkerGroupId {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct(type_name::<Self>())
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}

/// Transfers the registry-created identity and exact state together to a participation handle.
pub(crate) struct GroupRegistration {
    id: WorkerGroupId,
    state: Arc<WorkerGroupState>,
}

impl GroupRegistration {
    pub(crate) fn into_parts(self) -> (WorkerGroupId, Arc<WorkerGroupState>) {
        (self.id, self.state)
    }
}

/// Supplies the lifetime and synchronization domain for all public worker-group handles.
///
/// The unique owner makes pointer-bearing IDs pool-specific, while the retained runtime and
/// group handles keep every resolved state allocation alive for as long as the registry exists.
struct WorkerGroupsInner {
    owner: u64,
    runtime: Arc<PoolRuntime>,
    groups: Mutex<Vec<WorkerGroup>>,
    #[cfg(test)]
    before_membership_selection: OnceLock<Arc<dyn Fn() + Send + Sync + UnwindSafe + RefUnwindSafe>>,
}

#[cfg(test)]
/// Iterates copied group identities while retaining the registry borrow that owns their state.
pub(crate) struct InitializedGroups<'a> {
    groups: &'a WorkerGroups,
    ids: VecIntoIter<WorkerGroupId>,
}

#[cfg(test)]
impl<'a> Iterator for InitializedGroups<'a> {
    type Item = &'a WorkerGroupState;

    fn next(&mut self) -> Option<Self::Item> {
        self.ids.next().map(|id| self.groups.resolve(id))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use many_cpus::SystemHardware;
    use many_cpus::fake::HardwareBuilder;
    use new_zealand::nz;
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use testing::assert_panics;

    use super::*;
    use crate::Pool;

    assert_impl_all!(WorkerGroups: Clone, Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(
        WorkerGroup: Clone,
        Send,
        Sync,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_impl_all!(
        WorkerGroupId: Copy,
        Send,
        Sync,
        UnwindSafe,
        RefUnwindSafe
    );
    assert_not_impl_any!(WorkerGroups: Copy);
    assert_not_impl_any!(WorkerGroup: Copy);

    #[test]
    fn foreign_group_id_is_rejected() {
        let hardware = || SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
        let first = Pool::builder().hardware(hardware()).build().unwrap();
        let second = Pool::builder().hardware(hardware()).build().unwrap();
        let foreign = first.inner.groups.add().id();

        assert_panics(|| second.inner.groups.resolve(foreign));
    }

    #[test]
    fn registry_and_group_handles_format() {
        let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(1), nz!(1)));
        let pool = Pool::builder().hardware(hardware).build().unwrap();
        let group = pool.inner.groups.add();

        _ = format!("{:?}", pool.inner.groups);
        _ = format!("{group:?}");
        _ = format!("{:?}", group.id());
        let worker = group.add_worker();
        _ = format!("{worker:?}");
        drop(worker);
    }
}
