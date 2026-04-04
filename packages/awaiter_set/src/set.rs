use std::panic::{RefUnwindSafe, UnwindSafe};
use std::ptr;

use crate::AwaiterNode;

/// A set of pinned [`AwaiterNode`]s awaiting notification.
///
/// The set does not own its nodes. Nodes are embedded in async futures
/// and linked/unlinked as awaiters register and complete.
///
/// The set is `Send` but not `Sync` — it can be moved between threads
/// but all access must be serialized by the caller.
pub struct AwaiterSet {
    head: *mut AwaiterNode,
    tail: *mut AwaiterNode,
}

impl AwaiterSet {
    /// Creates a new empty set.
    ///
    /// # Examples
    ///
    /// ```
    /// use awaiter_set::AwaiterSet;
    ///
    /// let mut set = AwaiterSet::new();
    /// assert!(set.is_empty());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            head: ptr::null_mut(),
            tail: ptr::null_mut(),
        }
    }

    /// Returns `true` if the set contains no nodes.
    #[must_use]
    pub fn is_empty(&mut self) -> bool {
        self.head.is_null()
    }

    /// Returns a raw pointer to the next node to be taken.
    ///
    /// Returns a null pointer if the set is empty.
    #[must_use]
    pub fn head(&mut self) -> *mut AwaiterNode {
        self.head
    }

    /// Inserts a node into the set.
    ///
    /// # Safety
    ///
    /// * `node` must point to a valid, pinned [`AwaiterNode`] that is not
    ///   already in any set.
    pub unsafe fn insert(&mut self, node: *mut AwaiterNode) {
        // SAFETY: Caller guarantees `node` is valid.
        unsafe {
            (*node).next = ptr::null_mut();
        }

        // SAFETY: Caller guarantees `node` is valid.
        unsafe {
            (*node).prev = self.tail;
        }

        if self.tail.is_null() {
            self.head = node;
        } else {
            // SAFETY: `tail` is non-null and valid (set invariant).
            unsafe {
                (*self.tail).next = node;
            }
        }

        self.tail = node;
    }

    /// Removes a specific node from the set.
    ///
    /// After removal, the node's list pointers are cleared (set to null).
    ///
    /// # Safety
    ///
    /// * `node` must point to a valid, pinned [`AwaiterNode`] that is
    ///   currently in this set.
    pub unsafe fn remove(&mut self, node: *mut AwaiterNode) {
        // SAFETY: Caller guarantees `node` is valid and in the set.
        let prev = unsafe { (*node).prev };
        // SAFETY: Caller guarantees `node` is valid and in the set.
        let next = unsafe { (*node).next };

        if prev.is_null() {
            self.head = next;
        } else {
            // SAFETY: `prev` is a valid node in the set.
            unsafe {
                (*prev).next = next;
            }
        }

        if next.is_null() {
            self.tail = prev;
        } else {
            // SAFETY: `next` is a valid node in the set.
            unsafe {
                (*next).prev = prev;
            }
        }

        // Clear the removed node's links for safety.
        // SAFETY: Caller guarantees `node` is valid.
        unsafe {
            (*node).next = ptr::null_mut();
        }
        // SAFETY: Caller guarantees `node` is valid.
        unsafe {
            (*node).prev = ptr::null_mut();
        }
    }

    /// Removes and returns one node from the set.
    ///
    /// Returns `None` if the set is empty. The returned pointer points
    /// to a valid, pinned node that is no longer in the set.
    pub fn take_one(&mut self) -> Option<*mut AwaiterNode> {
        if self.head.is_null() {
            return None;
        }

        let node = self.head;

        // SAFETY: `head` is non-null, so it is a valid node.
        let next = unsafe { (*node).next };

        self.head = next;

        if next.is_null() {
            self.tail = ptr::null_mut();
        } else {
            // SAFETY: `next` is a valid node in the set.
            unsafe {
                (*next).prev = ptr::null_mut();
            }
        }

        // Clear the removed node's links.
        // SAFETY: `node` is valid (we just read from it above).
        unsafe {
            (*node).next = ptr::null_mut();
        }
        // SAFETY: `node` is valid.
        unsafe {
            (*node).prev = ptr::null_mut();
        }

        Some(node)
    }

    /// Iterates over all nodes, calling `f` for each.
    ///
    /// The next-pointer is read before invoking the callback, so the
    /// callback may safely modify the current node's waker or flags.
    /// However, it must not add or remove nodes from the set.
    pub fn for_each(&mut self, mut f: impl FnMut(*mut AwaiterNode)) {
        let mut current = self.head;
        while !current.is_null() {
            // Read next before calling `f` so we do not hold a reference
            // to the current node across the callback.
            // SAFETY: `current` is non-null and in the set.
            let next = unsafe { (*current).next };
            f(current);
            current = next;
        }
    }
}

impl Default for AwaiterSet {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: Raw pointers are `!Send` by default. This impl is sound because
// all pointer dereferences are serialized by external synchronization
// (Mutex or single-thread confinement). the set can safely be moved
// between threads as long as access remains exclusive.
unsafe impl Send for AwaiterSet {}

// AwaiterSet contains only raw pointers. All pointer
// dereferences are serialized by external synchronization, so no
// inconsistent state can be observed during unwind.
impl UnwindSafe for AwaiterSet {}
impl RefUnwindSafe for AwaiterSet {}

impl std::fmt::Debug for AwaiterSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("is_empty", &self.head.is_null())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block,
    clippy::indexing_slicing,
    reason = "test code with trivial safety invariants"
)]
mod tests {
    use std::task::{RawWaker, RawWakerVTable, Waker};

    use super::*;

    static_assertions::assert_impl_all!(AwaiterSet: Send, UnwindSafe, RefUnwindSafe);
    static_assertions::assert_not_impl_any!(AwaiterSet: Sync);

    fn noop_waker() -> Waker {
        fn clone(data: *const ()) -> RawWaker {
            RawWaker::new(data, &VTABLE)
        }
        fn noop(_: *const ()) {}
        static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
        unsafe { Waker::from_raw(RawWaker::new(ptr::null(), &VTABLE)) }
    }

    #[test]
    fn new_list_is_empty() {
        let mut list = AwaiterSet::new();
        assert!(list.is_empty());
        assert!(list.head().is_null());
    }

    #[test]
    fn default_list_is_empty() {
        let mut list = AwaiterSet::default();
        assert!(list.is_empty());
    }

    #[test]
    fn take_one_on_empty_returns_none() {
        let mut list = AwaiterSet::new();
        assert!(list.take_one().is_none());
    }

    #[test]
    fn push_and_pop_single_element() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let pa: *mut AwaiterNode = &raw mut a;

        unsafe {
            list.insert(pa);
        }
        assert!(!list.is_empty());
        assert!(ptr::eq(list.head(), pa));

        let popped = list.take_one();
        assert!(ptr::eq(popped.unwrap(), pa));
        assert!(list.is_empty());
    }

    #[test]
    fn fifo_ordering() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let mut b = AwaiterNode::new();
        let mut c = AwaiterNode::new();

        let pa: *mut AwaiterNode = &raw mut a;
        let pb: *mut AwaiterNode = &raw mut b;
        let pc: *mut AwaiterNode = &raw mut c;

        unsafe {
            list.insert(pa);
            list.insert(pb);
            list.insert(pc);
        }

        assert!(!list.is_empty());

        let first = list.take_one();
        assert!(ptr::eq(first.unwrap(), pa));

        let second = list.take_one();
        assert!(ptr::eq(second.unwrap(), pb));

        let third = list.take_one();
        assert!(ptr::eq(third.unwrap(), pc));

        assert!(list.is_empty());
        assert!(list.take_one().is_none());
    }

    #[test]
    fn remove_head_node() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let mut b = AwaiterNode::new();

        let pa: *mut AwaiterNode = &raw mut a;
        let pb: *mut AwaiterNode = &raw mut b;

        unsafe {
            list.insert(pa);
            list.insert(pb);
            list.remove(pa);
        }

        let first = list.take_one();
        assert!(ptr::eq(first.unwrap(), pb));
        assert!(list.is_empty());
    }

    #[test]
    fn remove_tail_node() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let mut b = AwaiterNode::new();

        let pa: *mut AwaiterNode = &raw mut a;
        let pb: *mut AwaiterNode = &raw mut b;

        unsafe {
            list.insert(pa);
            list.insert(pb);
            list.remove(pb);
        }

        let first = list.take_one();
        assert!(ptr::eq(first.unwrap(), pa));
        assert!(list.is_empty());
    }

    #[test]
    fn remove_middle_node() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let mut b = AwaiterNode::new();
        let mut c = AwaiterNode::new();

        let pa: *mut AwaiterNode = &raw mut a;
        let pb: *mut AwaiterNode = &raw mut b;
        let pc: *mut AwaiterNode = &raw mut c;

        unsafe {
            list.insert(pa);
            list.insert(pb);
            list.insert(pc);
            list.remove(pb);
        }

        let first = list.take_one();
        assert!(ptr::eq(first.unwrap(), pa));

        let second = list.take_one();
        assert!(ptr::eq(second.unwrap(), pc));

        assert!(list.is_empty());
    }

    #[test]
    fn remove_only_node() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let pa: *mut AwaiterNode = &raw mut a;

        unsafe {
            list.insert(pa);
            list.remove(pa);
        }

        assert!(list.is_empty());
    }

    #[test]
    fn remove_clears_node_links() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let mut b = AwaiterNode::new();
        let pa: *mut AwaiterNode = &raw mut a;
        let pb: *mut AwaiterNode = &raw mut b;

        unsafe {
            list.insert(pa);
            list.insert(pb);
            list.remove(pa);
        }

        assert!(a.next_in_list().is_null());
    }

    #[test]
    fn reuse_after_removal() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let mut b = AwaiterNode::new();
        let pa: *mut AwaiterNode = &raw mut a;
        let pb: *mut AwaiterNode = &raw mut b;

        unsafe {
            list.insert(pa);
            list.remove(pa);
            list.insert(pb);
        }

        let popped = list.take_one();
        assert!(ptr::eq(popped.unwrap(), pb));
        assert!(list.is_empty());
    }

    #[test]
    fn interleaved_push_and_pop() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let mut b = AwaiterNode::new();
        let mut c = AwaiterNode::new();

        let pa: *mut AwaiterNode = &raw mut a;
        let pb: *mut AwaiterNode = &raw mut b;
        let pc: *mut AwaiterNode = &raw mut c;

        unsafe {
            list.insert(pa);
        }
        unsafe {
            list.insert(pb);
        }

        let first = list.take_one();
        assert!(ptr::eq(first.unwrap(), pa));

        unsafe {
            list.insert(pc);
        }

        let second = list.take_one();
        assert!(ptr::eq(second.unwrap(), pb));

        let third = list.take_one();
        assert!(ptr::eq(third.unwrap(), pc));

        assert!(list.is_empty());
    }

    #[test]
    fn traversal_via_next_in_list() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let mut b = AwaiterNode::new();
        let mut c = AwaiterNode::new();

        let pa: *mut AwaiterNode = &raw mut a;
        let pb: *mut AwaiterNode = &raw mut b;
        let pc: *mut AwaiterNode = &raw mut c;

        unsafe {
            list.insert(pa);
            list.insert(pb);
            list.insert(pc);
        }

        let head = list.head();
        assert!(ptr::eq(head, pa));

        let second = unsafe { (*head).next_in_list() };
        assert!(ptr::eq(second, pb));

        let third = unsafe { (*second).next_in_list() };
        assert!(ptr::eq(third, pc));

        let end = unsafe { (*third).next_in_list() };
        assert!(end.is_null());
    }

    #[test]
    fn for_each_visits_all_nodes_in_order() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let mut b = AwaiterNode::new();

        let pa: *mut AwaiterNode = &raw mut a;
        let pb: *mut AwaiterNode = &raw mut b;

        unsafe {
            list.insert(pa);
            list.insert(pb);
        }

        let mut visited = Vec::new();
        list.for_each(|node| visited.push(node));

        assert_eq!(visited.len(), 2);
        assert!(ptr::eq(visited[0], pa));
        assert!(ptr::eq(visited[1], pb));
    }

    #[test]
    fn for_each_on_empty_list_does_nothing() {
        let mut list = AwaiterSet::new();
        let mut count = 0_usize;
        list.for_each(|_| count = count.checked_add(1).unwrap());
        assert_eq!(count, 0);
    }

    #[test]
    fn wakers_survive_list_operations() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let pa: *mut AwaiterNode = &raw mut a;

        let waker = noop_waker();
        a.store_waker(waker);

        unsafe {
            list.insert(pa);
        }

        let popped = list.take_one().unwrap();
        let recovered = unsafe { (*popped).take_waker() };
        assert!(recovered.is_some());
    }

    #[test]
    fn user_data_survives_list_operations() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let pa: *mut AwaiterNode = &raw mut a;

        a.set_user_data(7);
        unsafe {
            list.insert(pa);
        }

        let popped = list.take_one().unwrap();
        assert_eq!(unsafe { (*popped).user_data() }, 7);
    }

    #[test]
    fn notified_flag_survives_list_operations() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let pa: *mut AwaiterNode = &raw mut a;

        unsafe {
            list.insert(pa);
        }

        let popped = list.take_one().unwrap();
        unsafe {
            (*popped).set_notified();
        }
        assert!(unsafe { (*popped).is_notified() });
    }

    #[test]
    fn ten_elements_maintain_fifo() {
        let mut list = AwaiterSet::new();
        let mut nodes: Vec<AwaiterNode> =
            std::iter::repeat_with(AwaiterNode::new).take(10).collect();
        let ptrs: Vec<*mut AwaiterNode> = nodes.iter_mut().map(ptr::from_mut).collect();

        for &p in &ptrs {
            unsafe {
                list.insert(p);
            }
        }

        for &expected in &ptrs {
            let popped = list.take_one().unwrap();
            assert!(ptr::eq(popped, expected));
        }

        assert!(list.is_empty());
    }

    #[test]
    fn debug_output_does_not_panic() {
        let list = AwaiterSet::new();
        let debug = format!("{list:?}");
        assert!(debug.contains("AwaiterSet"));
    }
}
