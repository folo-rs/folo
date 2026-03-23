use std::panic::{RefUnwindSafe, UnwindSafe};

use crate::WaiterNode;

/// A FIFO doubly-linked list of pinned [`WaiterNode`]s.
///
/// The list does not own its nodes. Nodes are embedded in async futures and
/// linked/unlinked as waiters register and complete. All operations that
/// modify the list are `unsafe` because they require the caller to uphold
/// pointer validity and exclusive-access invariants.
///
/// # Synchronization
///
/// `WaiterList` has no internal synchronization. The caller must ensure
/// exclusive access for every operation — typically by holding a
/// [`Mutex`][std::sync::Mutex] guard or by confining the list to a single
/// thread via `!Send` on the containing type.
///
/// # Complexity
///
/// All operations are O(1):
///
/// | Operation | Description |
/// |---|---|
/// | [`push_back`][Self::push_back] | Append to tail |
/// | [`pop_front`][Self::pop_front] | Remove from head |
/// | [`remove`][Self::remove] | Remove arbitrary node |
/// | [`head`][Self::head] | Read head pointer |
pub struct WaiterList {
    head: *mut WaiterNode,
    tail: *mut WaiterNode,
}

impl WaiterList {
    /// Creates a new empty list.
    ///
    /// # Examples
    ///
    /// ```
    /// use waiter_list::WaiterList;
    ///
    /// let list = WaiterList::new();
    /// assert!(list.is_empty());
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            head: std::ptr::null_mut(),
            tail: std::ptr::null_mut(),
        }
    }

    /// Returns `true` if the list contains no nodes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.head.is_null()
    }

    /// Returns a raw pointer to the head (front) node.
    ///
    /// Returns a null pointer if the list is empty. The pointer is valid
    /// only as long as the caller maintains exclusive access to the list.
    #[must_use]
    pub fn head(&self) -> *mut WaiterNode {
        self.head
    }

    /// Appends a node to the back of the list (FIFO enqueue).
    ///
    /// # Safety
    ///
    /// * `node` must point to a valid, pinned [`WaiterNode`] that is not
    ///   already in any list.
    /// * The caller must ensure exclusive access to the list.
    pub unsafe fn push_back(&mut self, node: *mut WaiterNode) {
        // SAFETY: Caller guarantees `node` is valid.
        unsafe {
            (*node).next = std::ptr::null_mut();
        }

        // SAFETY: Caller guarantees `node` is valid.
        unsafe {
            (*node).prev = self.tail;
        }

        if self.tail.is_null() {
            self.head = node;
        } else {
            // SAFETY: `tail` is non-null and valid (list invariant).
            unsafe {
                (*self.tail).next = node;
            }
        }

        self.tail = node;
    }

    /// Removes a specific node from the list.
    ///
    /// After removal, the node's list pointers are cleared (set to null).
    ///
    /// # Safety
    ///
    /// * `node` must point to a valid, pinned [`WaiterNode`] that is
    ///   currently in this list.
    /// * The caller must ensure exclusive access to the list.
    pub unsafe fn remove(&mut self, node: *mut WaiterNode) {
        // SAFETY: Caller guarantees `node` is valid and in the list.
        let prev = unsafe { (*node).prev };
        // SAFETY: Caller guarantees `node` is valid and in the list.
        let next = unsafe { (*node).next };

        if prev.is_null() {
            self.head = next;
        } else {
            // SAFETY: `prev` is a valid node in the list.
            unsafe {
                (*prev).next = next;
            }
        }

        if next.is_null() {
            self.tail = prev;
        } else {
            // SAFETY: `next` is a valid node in the list.
            unsafe {
                (*next).prev = prev;
            }
        }

        // Clear the removed node's links for safety.
        // SAFETY: Caller guarantees `node` is valid.
        unsafe {
            (*node).next = std::ptr::null_mut();
        }
        // SAFETY: Caller guarantees `node` is valid.
        unsafe {
            (*node).prev = std::ptr::null_mut();
        }
    }

    /// Removes and returns the front node (FIFO dequeue).
    ///
    /// Returns `None` if the list is empty.
    ///
    /// # Safety
    ///
    /// * The caller must ensure exclusive access to the list.
    /// * Any returned pointer is valid and pinned; the node is no longer
    ///   in the list after this call.
    pub unsafe fn pop_front(&mut self) -> Option<*mut WaiterNode> {
        if self.head.is_null() {
            return None;
        }

        let node = self.head;

        // SAFETY: `head` is non-null, so it is a valid node.
        let next = unsafe { (*node).next };

        self.head = next;

        if next.is_null() {
            self.tail = std::ptr::null_mut();
        } else {
            // SAFETY: `next` is a valid node in the list.
            unsafe {
                (*next).prev = std::ptr::null_mut();
            }
        }

        // Clear the removed node's links.
        // SAFETY: `node` is valid (we just read from it above).
        unsafe {
            (*node).next = std::ptr::null_mut();
        }
        // SAFETY: `node` is valid.
        unsafe {
            (*node).prev = std::ptr::null_mut();
        }

        Some(node)
    }

    /// Iterates over all nodes front-to-back, calling `f` for each.
    ///
    /// The next-pointer is read before invoking the callback, so the
    /// callback may safely modify the current node's waker or flags.
    /// However, it must not add or remove nodes from the list.
    ///
    /// # Safety
    ///
    /// * The caller must ensure exclusive access to the list.
    /// * The callback must not modify the list structure (no push/pop/remove).
    pub unsafe fn for_each(&self, mut f: impl FnMut(*mut WaiterNode)) {
        let mut current = self.head;
        while !current.is_null() {
            // Read next before calling `f` so we do not hold a reference
            // to the current node across the callback.
            // SAFETY: `current` is non-null and in the list.
            let next = unsafe { (*current).next };
            f(current);
            current = next;
        }
    }
}

impl Default for WaiterList {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: Raw pointers are `!Send` by default. This impl is sound because
// all pointer dereferences are serialized by external synchronization
// (Mutex or single-thread confinement). The list can safely be moved
// between threads as long as access remains exclusive.
unsafe impl Send for WaiterList {}

impl UnwindSafe for WaiterList {}
impl RefUnwindSafe for WaiterList {}

impl std::fmt::Debug for WaiterList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaiterList")
            .field("is_empty", &self.is_empty())
            .finish()
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

    static_assertions::assert_impl_all!(WaiterList: Send, UnwindSafe, RefUnwindSafe);
    static_assertions::assert_not_impl_any!(WaiterList: Sync);

    fn noop_waker() -> Waker {
        fn clone(data: *const ()) -> RawWaker {
            RawWaker::new(data, &VTABLE)
        }
        fn noop(_: *const ()) {}
        static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
        unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
    }

    #[test]
    fn new_list_is_empty() {
        let list = WaiterList::new();
        assert!(list.is_empty());
        assert!(list.head().is_null());
    }

    #[test]
    fn default_list_is_empty() {
        let list = WaiterList::default();
        assert!(list.is_empty());
    }

    #[test]
    fn pop_front_on_empty_returns_none() {
        let mut list = WaiterList::new();
        assert!(unsafe { list.pop_front() }.is_none());
    }

    #[test]
    fn push_and_pop_single_element() {
        let mut list = WaiterList::new();
        let mut a = WaiterNode::new();
        let pa: *mut WaiterNode = &raw mut a;

        unsafe {
            list.push_back(pa);
        }
        assert!(!list.is_empty());
        assert!(std::ptr::eq(list.head(), pa));

        let popped = unsafe { list.pop_front() };
        assert!(std::ptr::eq(popped.unwrap(), pa));
        assert!(list.is_empty());
    }

    #[test]
    fn fifo_ordering() {
        let mut list = WaiterList::new();
        let mut a = WaiterNode::new();
        let mut b = WaiterNode::new();
        let mut c = WaiterNode::new();

        let pa: *mut WaiterNode = &raw mut a;
        let pb: *mut WaiterNode = &raw mut b;
        let pc: *mut WaiterNode = &raw mut c;

        unsafe {
            list.push_back(pa);
            list.push_back(pb);
            list.push_back(pc);
        }

        assert!(!list.is_empty());

        let first = unsafe { list.pop_front() };
        assert!(std::ptr::eq(first.unwrap(), pa));

        let second = unsafe { list.pop_front() };
        assert!(std::ptr::eq(second.unwrap(), pb));

        let third = unsafe { list.pop_front() };
        assert!(std::ptr::eq(third.unwrap(), pc));

        assert!(list.is_empty());
        assert!(unsafe { list.pop_front() }.is_none());
    }

    #[test]
    fn remove_head_node() {
        let mut list = WaiterList::new();
        let mut a = WaiterNode::new();
        let mut b = WaiterNode::new();

        let pa: *mut WaiterNode = &raw mut a;
        let pb: *mut WaiterNode = &raw mut b;

        unsafe {
            list.push_back(pa);
            list.push_back(pb);
            list.remove(pa);
        }

        let first = unsafe { list.pop_front() };
        assert!(std::ptr::eq(first.unwrap(), pb));
        assert!(list.is_empty());
    }

    #[test]
    fn remove_tail_node() {
        let mut list = WaiterList::new();
        let mut a = WaiterNode::new();
        let mut b = WaiterNode::new();

        let pa: *mut WaiterNode = &raw mut a;
        let pb: *mut WaiterNode = &raw mut b;

        unsafe {
            list.push_back(pa);
            list.push_back(pb);
            list.remove(pb);
        }

        let first = unsafe { list.pop_front() };
        assert!(std::ptr::eq(first.unwrap(), pa));
        assert!(list.is_empty());
    }

    #[test]
    fn remove_middle_node() {
        let mut list = WaiterList::new();
        let mut a = WaiterNode::new();
        let mut b = WaiterNode::new();
        let mut c = WaiterNode::new();

        let pa: *mut WaiterNode = &raw mut a;
        let pb: *mut WaiterNode = &raw mut b;
        let pc: *mut WaiterNode = &raw mut c;

        unsafe {
            list.push_back(pa);
            list.push_back(pb);
            list.push_back(pc);
            list.remove(pb);
        }

        let first = unsafe { list.pop_front() };
        assert!(std::ptr::eq(first.unwrap(), pa));

        let second = unsafe { list.pop_front() };
        assert!(std::ptr::eq(second.unwrap(), pc));

        assert!(list.is_empty());
    }

    #[test]
    fn remove_only_node() {
        let mut list = WaiterList::new();
        let mut a = WaiterNode::new();
        let pa: *mut WaiterNode = &raw mut a;

        unsafe {
            list.push_back(pa);
            list.remove(pa);
        }

        assert!(list.is_empty());
    }

    #[test]
    fn remove_clears_node_links() {
        let mut list = WaiterList::new();
        let mut a = WaiterNode::new();
        let mut b = WaiterNode::new();
        let pa: *mut WaiterNode = &raw mut a;
        let pb: *mut WaiterNode = &raw mut b;

        unsafe {
            list.push_back(pa);
            list.push_back(pb);
            list.remove(pa);
        }

        assert!(a.next_in_list().is_null());
    }

    #[test]
    fn reuse_after_removal() {
        let mut list = WaiterList::new();
        let mut a = WaiterNode::new();
        let mut b = WaiterNode::new();
        let pa: *mut WaiterNode = &raw mut a;
        let pb: *mut WaiterNode = &raw mut b;

        unsafe {
            list.push_back(pa);
            list.remove(pa);
            list.push_back(pb);
        }

        let popped = unsafe { list.pop_front() };
        assert!(std::ptr::eq(popped.unwrap(), pb));
        assert!(list.is_empty());
    }

    #[test]
    fn interleaved_push_and_pop() {
        let mut list = WaiterList::new();
        let mut a = WaiterNode::new();
        let mut b = WaiterNode::new();
        let mut c = WaiterNode::new();

        let pa: *mut WaiterNode = &raw mut a;
        let pb: *mut WaiterNode = &raw mut b;
        let pc: *mut WaiterNode = &raw mut c;

        unsafe {
            list.push_back(pa);
        }
        unsafe {
            list.push_back(pb);
        }

        let first = unsafe { list.pop_front() };
        assert!(std::ptr::eq(first.unwrap(), pa));

        unsafe {
            list.push_back(pc);
        }

        let second = unsafe { list.pop_front() };
        assert!(std::ptr::eq(second.unwrap(), pb));

        let third = unsafe { list.pop_front() };
        assert!(std::ptr::eq(third.unwrap(), pc));

        assert!(list.is_empty());
    }

    #[test]
    fn traversal_via_next_in_list() {
        let mut list = WaiterList::new();
        let mut a = WaiterNode::new();
        let mut b = WaiterNode::new();
        let mut c = WaiterNode::new();

        let pa: *mut WaiterNode = &raw mut a;
        let pb: *mut WaiterNode = &raw mut b;
        let pc: *mut WaiterNode = &raw mut c;

        unsafe {
            list.push_back(pa);
            list.push_back(pb);
            list.push_back(pc);
        }

        let head = list.head();
        assert!(std::ptr::eq(head, pa));

        let second = unsafe { (*head).next_in_list() };
        assert!(std::ptr::eq(second, pb));

        let third = unsafe { (*second).next_in_list() };
        assert!(std::ptr::eq(third, pc));

        let end = unsafe { (*third).next_in_list() };
        assert!(end.is_null());
    }

    #[test]
    fn for_each_visits_all_nodes_in_order() {
        let mut list = WaiterList::new();
        let mut a = WaiterNode::new();
        let mut b = WaiterNode::new();

        let pa: *mut WaiterNode = &raw mut a;
        let pb: *mut WaiterNode = &raw mut b;

        unsafe {
            list.push_back(pa);
            list.push_back(pb);
        }

        let mut visited = Vec::new();
        unsafe {
            list.for_each(|node| visited.push(node));
        }

        assert_eq!(visited.len(), 2);
        assert!(std::ptr::eq(visited[0], pa));
        assert!(std::ptr::eq(visited[1], pb));
    }

    #[test]
    fn for_each_on_empty_list_does_nothing() {
        let list = WaiterList::new();
        let mut count = 0_usize;
        unsafe {
            list.for_each(|_| count = count.checked_add(1).unwrap());
        }
        assert_eq!(count, 0);
    }

    #[test]
    fn wakers_survive_list_operations() {
        let mut list = WaiterList::new();
        let mut a = WaiterNode::new();
        let pa: *mut WaiterNode = &raw mut a;

        let waker = noop_waker();
        a.store_waker(&waker);

        unsafe {
            list.push_back(pa);
        }

        let popped = unsafe { list.pop_front() }.unwrap();
        let recovered = unsafe { (*popped).take_waker() };
        assert!(recovered.is_some());
    }

    #[test]
    fn user_data_survives_list_operations() {
        let mut list = WaiterList::new();
        let mut a = WaiterNode::new();
        let pa: *mut WaiterNode = &raw mut a;

        a.set_user_data(7);
        unsafe {
            list.push_back(pa);
        }

        let popped = unsafe { list.pop_front() }.unwrap();
        assert_eq!(unsafe { (*popped).user_data() }, 7);
    }

    #[test]
    fn notified_flag_survives_list_operations() {
        let mut list = WaiterList::new();
        let mut a = WaiterNode::new();
        let pa: *mut WaiterNode = &raw mut a;

        unsafe {
            list.push_back(pa);
        }

        let popped = unsafe { list.pop_front() }.unwrap();
        unsafe {
            (*popped).set_notified();
        }
        assert!(unsafe { (*popped).is_notified() });
    }

    #[test]
    fn ten_elements_maintain_fifo() {
        let mut list = WaiterList::new();
        let mut nodes: Vec<WaiterNode> = std::iter::repeat_with(WaiterNode::new).take(10).collect();
        let ptrs: Vec<*mut WaiterNode> = nodes.iter_mut().map(std::ptr::from_mut).collect();

        for &p in &ptrs {
            unsafe {
                list.push_back(p);
            }
        }

        for &expected in &ptrs {
            let popped = unsafe { list.pop_front() }.unwrap();
            assert!(std::ptr::eq(popped, expected));
        }

        assert!(list.is_empty());
    }

    #[test]
    fn debug_output_does_not_panic() {
        let list = WaiterList::new();
        let debug = format!("{list:?}");
        assert!(debug.contains("WaiterList"));
    }
}
