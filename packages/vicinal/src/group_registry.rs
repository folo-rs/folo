//! Dynamic registry and copyable routing identities for worker groups.

use std::any::type_name;
use std::fmt::{Debug, Formatter, Result as FmtResult};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::{
    GroupId, GroupState, NEVER_POISONED, PoolRuntime, WorkerGroup, WorkerGroupId, WorkerGroupState,
};

// Identity values need uniqueness only; the registry-specific validation supplies the
// synchronization relationship.
static NEXT_REGISTRY_ID: AtomicU64 = AtomicU64::new(1);

/// A cloneable collection of worker groups owned by one pool.
///
/// Cloning this collection does not clone or start any workers. Policies may retain
/// a clone to add groups later.
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
            .expect("worker registry identity space must not wrap");
        runtime.set_registry_owner(owner);

        Self {
            inner: Arc::new(WorkerGroupsInner {
                owner,
                runtime,
                groups: Mutex::new(Vec::new()),
            }),
        }
    }

    /// Adds one empty group and returns a cloneable participation handle.
    ///
    /// Use this during policy setup or when adding capacity. Cloning the returned handle
    /// into newly spawned worker threads is expected; task routing should return
    /// [`WorkerGroupId`] instead.
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
        let group = WorkerGroup::new(Arc::clone(&self.inner.runtime), destination);

        let mut groups = self.inner.groups.lock().expect(NEVER_POISONED);
        groups.push(group.clone());

        group
    }

    pub(crate) fn add_for(&self, destination: GroupId) -> WorkerGroup {
        self.add_inner(Some(destination))
    }

    /// Returns the number of groups currently present.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.groups.lock().expect(NEVER_POISONED).len()
    }

    /// Returns whether no groups have been added.
    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns a snapshot of every group for inspection.
    ///
    /// The snapshot clones every stored group handle. Policies that choose a group for each
    /// task should cache [`WorkerGroupId`] values instead of taking snapshots repeatedly.
    #[must_use]
    #[inline]
    pub fn iter(&self) -> std::vec::IntoIter<WorkerGroup> {
        self.snapshot().into_iter()
    }

    pub(crate) fn resolve(&self, group: WorkerGroupId) -> &WorkerGroupState {
        assert!(
            group.owner == self.inner.owner,
            "worker group belongs to another pool"
        );

        // IDs contain stable addresses owned by this registry, and group removal is not
        // supported. The returned reference cannot outlive the registry borrow.
        // SAFETY: The pointer is aligned to an initialized WorkerGroupState in this
        // registry-owned Arc allocation. Owner validation prevents cross-registry reuse, groups
        // are never removed or relocated, and the registry borrow outlives the shared reference.
        // The registry and every WorkerGroup retain the state only through Arc. Their mutable
        // fields use interior synchronization, so no &mut WorkerGroupState exists while this &T
        // is borrowed.
        let state: &GroupState = unsafe { group.state.as_ref() };
        state
    }

    pub(crate) fn snapshot(&self) -> Vec<WorkerGroup> {
        self.inner.groups.lock().expect(NEVER_POISONED).clone()
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
            .expect("group ID belongs to this registry")
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
    type IntoIter = std::vec::IntoIter<WorkerGroup>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl IntoIterator for WorkerGroups {
    type Item = WorkerGroup;
    type IntoIter = std::vec::IntoIter<WorkerGroup>;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// Retains one pool's unique owner, runtime and append-only group storage.
struct WorkerGroupsInner {
    owner: u64,
    runtime: Arc<PoolRuntime>,
    groups: Mutex<Vec<WorkerGroup>>,
}

#[cfg(test)]
/// Iterates copied group identities while retaining the registry borrow that owns their state.
pub(crate) struct InitializedGroups<'a> {
    groups: &'a WorkerGroups,
    ids: std::vec::IntoIter<WorkerGroupId>,
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
    }
}
