//! Dynamic registry and copyable routing identities for worker groups.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::{GroupId, GroupState, NEVER_POISONED, PoolRuntime, WorkerGroup, WorkerGroupId};

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
        let owner = NEXT_REGISTRY_ID.fetch_add(1, Ordering::Relaxed);
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
    /// This is a cold operation. Cloning the returned handle into newly spawned worker
    /// threads is expected; task routing should return [`WorkerGroupId`] instead.
    ///
    /// # Panics
    ///
    /// Panics after the pool has permanently failed. A cleanly shut-down registry can
    /// still be used by a policy to construct and immediately abandon a submission.
    #[must_use]
    pub fn add(&self) -> WorkerGroup {
        self.add_inner(None)
    }

    fn add_inner(&self, destination: Option<GroupId>) -> WorkerGroup {
        let group = WorkerGroup::new(Arc::clone(&self.inner.runtime), destination);
        self.inner.runtime.register_group(group.state());

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
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Returns a cloned snapshot of every group.
    ///
    /// This is a cold/introspection operation and changes reference counts for the
    /// returned handles. Policies should cache [`WorkerGroupId`] values for hot routing.
    #[must_use]
    pub fn iter(&self) -> std::vec::IntoIter<WorkerGroup> {
        self.inner
            .groups
            .lock()
            .expect(NEVER_POISONED)
            .clone()
            .into_iter()
    }

    pub(crate) fn resolve(&self, group: WorkerGroupId) -> &GroupState {
        assert!(
            group.owner == self.inner.owner,
            "worker group belongs to another pool"
        );

        // IDs contain stable addresses owned by this registry, and group removal is not
        // supported. The returned reference cannot outlive the registry borrow.
        // SAFETY: IDs can only be created for this registry-owned Arc allocation. Groups are
        // never removed or relocated, and the returned reference is tied to this registry.
        unsafe { group.state.as_ref() }
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
    pub(crate) fn get_or_init(&self, destination: GroupId) -> &GroupState {
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

impl fmt::Debug for WorkerGroups {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkerGroups")
            .field("len", &self.len())
            .finish_non_exhaustive()
    }
}

impl IntoIterator for &WorkerGroups {
    type Item = WorkerGroup;
    type IntoIter = std::vec::IntoIter<WorkerGroup>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl IntoIterator for WorkerGroups {
    type Item = WorkerGroup;
    type IntoIter = std::vec::IntoIter<WorkerGroup>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

struct WorkerGroupsInner {
    owner: u64,
    runtime: Arc<PoolRuntime>,
    groups: Mutex<Vec<WorkerGroup>>,
}

#[cfg(test)]
pub(crate) struct InitializedGroups<'a> {
    groups: &'a WorkerGroups,
    ids: std::vec::IntoIter<WorkerGroupId>,
}

#[cfg(test)]
impl<'a> Iterator for InitializedGroups<'a> {
    type Item = &'a GroupState;

    fn next(&mut self) -> Option<Self::Item> {
        self.ids.next().map(|id| self.groups.resolve(id))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

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
        let first = Pool::builder().build();
        let second = Pool::builder().build();
        let foreign = first.inner.groups.add().id();

        assert_panics(|| second.inner.groups.resolve(foreign));
    }
}
