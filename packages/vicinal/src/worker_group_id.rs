//! Copyable identities for policy-selected worker groups.

use std::any::type_name;
use std::fmt::{Debug, Formatter, Result as FmtResult};
use std::ptr::NonNull;
use std::sync::Arc;

use crate::WorkerGroupState;

/// Identifies a worker group within one
/// [`WorkerGroups`][crate::worker_policy::WorkerGroups] collection.
///
/// This value is [`Copy`], so policies can route tasks without cloning a group handle.
/// IDs from another pool are rejected when the pool resolves the selected group.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct WorkerGroupId {
    pub(crate) owner: u64,
    /// Stable address used by the owning registry for lock-free, ref-count-free resolution.
    ///
    /// An index into the registry's `Vec<WorkerGroup>` would be an alternative safe identity.
    /// Resolving it would require locking that vector and cloning its `Arc` on every submission
    /// because the vector may grow and references to its elements cannot outlive its guard.
    /// Each state is instead independently heap-allocated and retained for the registry's
    /// lifetime, so only the pointer and the registry owner check cross the routing boundary.
    pub(crate) state: NonNull<WorkerGroupState>,
}

impl WorkerGroupId {
    pub(crate) fn new(owner: u64, state: &Arc<WorkerGroupState>) -> Self {
        Self {
            owner,
            state: NonNull::from(Arc::as_ref(state)),
        }
    }
}

// SAFETY: Moving an ID copies only its owner and address; the ID does not dereference or own
// the pointee. The pointer is dereferenced only by WorkerGroups::resolve, after checking the
// registry owner. That registry retains the Arc-backed state for the returned shared reference's
// entire lifetime. Future direct dereferences must preserve the same checked-resolution boundary.
unsafe impl Send for WorkerGroupId {}

// SAFETY: Sharing an ID copies only its owner and address. No operation on the ID reads the
// pointee: WorkerGroups::resolve performs the sole dereference after owner validation and under
// the registry borrow that keeps the Arc-backed state alive. Sharing the ID itself neither prolongs
// nor assumes the pointee's lifetime.
unsafe impl Sync for WorkerGroupId {}

impl Debug for WorkerGroupId {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        f.debug_struct(type_name::<Self>())
            .field("owner", &self.owner)
            .finish_non_exhaustive()
    }
}
