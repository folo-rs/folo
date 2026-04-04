use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
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

    /// Returns a shared reference to the next node to be taken.
    ///
    /// Returns `None` if the set is empty.
    #[must_use]
    pub fn peek(&mut self) -> Option<&AwaiterNode> {
        if self.head.is_null() {
            None
        } else {
            // SAFETY: All pointers in the set are valid and pinned,
            // an invariant established by `insert`. The `&mut self`
            // borrow prevents concurrent mutation.
            Some(unsafe { &*self.head })
        }
    }

    /// Inserts a node into the set.
    ///
    /// # Safety
    ///
    /// The node must remain valid and pinned at its current address
    /// until it is removed from the set (via [`take_one()`][Self::take_one]
    /// or [`remove()`][Self::remove]).
    pub unsafe fn insert(&mut self, mut node: Pin<&mut AwaiterNode>) {
        // SAFETY: We do not move the node; we only store the pointer.
        let ptr: *mut AwaiterNode = unsafe { node.as_mut().get_unchecked_mut() };

        // SAFETY: We just obtained this pointer from a valid Pin.
        unsafe {
            (*ptr).next = ptr::null_mut();
        }

        // SAFETY: Same pointer.
        unsafe {
            (*ptr).prev = self.tail;
        }

        if self.tail.is_null() {
            self.head = ptr;
        } else {
            // SAFETY: `tail` is non-null and valid (set invariant).
            unsafe {
                (*self.tail).next = ptr;
            }
        }

        self.tail = ptr;
    }

    /// Removes a specific node from the set.
    ///
    /// After removal, the node's internal pointers are cleared.
    ///
    /// # Safety
    ///
    /// The node must currently be in this set (not in a different set
    /// or unregistered).
    pub unsafe fn remove(&mut self, mut node: Pin<&mut AwaiterNode>) {
        // SAFETY: We do not move the node.
        let ptr: *mut AwaiterNode = unsafe { node.as_mut().get_unchecked_mut() };
        // SAFETY: We have Pin<&mut> proving the node is valid.
        let prev = unsafe { (*ptr).prev };
        // SAFETY: Same pointer.
        let next = unsafe { (*ptr).next };

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

        // Clear the removed node's links.
        // SAFETY: The node is valid (we hold Pin<&mut>).
        unsafe {
            (*ptr).next = ptr::null_mut();
        }
        // SAFETY: Same pointer.
        unsafe {
            (*ptr).prev = ptr::null_mut();
        }
    }

    /// Removes and returns one node from the set.
    ///
    /// Returns `None` if the set is empty.
    pub fn take_one(&mut self) -> Option<&mut AwaiterNode> {
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

        // SAFETY: The node is valid (inserted via `insert` which
        // requires the node to remain valid until removal). We just
        // removed it, so no other reference exists in the set. The
        // `&mut self` borrow prevents concurrent set operations.
        Some(unsafe { &mut *node })
    }

    /// Iterates over all nodes, calling `f` for each.
    ///
    /// The callback receives an exclusive reference to each node and
    /// may modify the node's waker or flags. However, it must not
    /// insert into or remove from the set.
    pub fn for_each(&mut self, mut f: impl FnMut(&mut AwaiterNode)) {
        let mut current = self.head;
        while !current.is_null() {
            // Read next before calling `f` so the callback cannot
            // invalidate our traversal pointer.
            // SAFETY: `current` is non-null and in the set.
            let next = unsafe { (*current).next };
            // SAFETY: The node is valid (set invariant from insert).
            // The `&mut self` borrow prevents concurrent set mutation.
            f(unsafe { &mut *current });
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
        assert!(list.peek().is_none());
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
        a.set_user_data(1);

        unsafe {
            list.insert(Pin::new_unchecked(&mut a));
        }
        assert!(!list.is_empty());
        assert_eq!(list.peek().unwrap().user_data(), 1);

        let popped = list.take_one();
        assert_eq!(popped.unwrap().user_data(), 1);
        assert!(list.is_empty());
    }

    #[test]
    fn fifo_ordering() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let mut b = AwaiterNode::new();
        let mut c = AwaiterNode::new();

        a.set_user_data(1);
        b.set_user_data(2);
        c.set_user_data(3);

        unsafe {
            list.insert(Pin::new_unchecked(&mut a));
            list.insert(Pin::new_unchecked(&mut b));
            list.insert(Pin::new_unchecked(&mut c));
        }

        assert!(!list.is_empty());

        assert_eq!(list.take_one().unwrap().user_data(), 1);
        assert_eq!(list.take_one().unwrap().user_data(), 2);
        assert_eq!(list.take_one().unwrap().user_data(), 3);

        assert!(list.is_empty());
        assert!(list.take_one().is_none());
    }

    #[test]
    fn remove_head_node() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let mut b = AwaiterNode::new();

        a.set_user_data(1);
        b.set_user_data(2);

        unsafe {
            list.insert(Pin::new_unchecked(&mut a));
            list.insert(Pin::new_unchecked(&mut b));
            list.remove(Pin::new_unchecked(&mut a));
        }

        assert_eq!(list.take_one().unwrap().user_data(), 2);
        assert!(list.is_empty());
    }

    #[test]
    fn remove_tail_node() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let mut b = AwaiterNode::new();

        a.set_user_data(1);
        b.set_user_data(2);

        unsafe {
            list.insert(Pin::new_unchecked(&mut a));
            list.insert(Pin::new_unchecked(&mut b));
            list.remove(Pin::new_unchecked(&mut b));
        }

        assert_eq!(list.take_one().unwrap().user_data(), 1);
        assert!(list.is_empty());
    }

    #[test]
    fn remove_middle_node() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let mut b = AwaiterNode::new();
        let mut c = AwaiterNode::new();

        a.set_user_data(1);
        b.set_user_data(2);
        c.set_user_data(3);

        unsafe {
            list.insert(Pin::new_unchecked(&mut a));
            list.insert(Pin::new_unchecked(&mut b));
            list.insert(Pin::new_unchecked(&mut c));
            list.remove(Pin::new_unchecked(&mut b));
        }

        assert_eq!(list.take_one().unwrap().user_data(), 1);
        assert_eq!(list.take_one().unwrap().user_data(), 3);
        assert!(list.is_empty());
    }

    #[test]
    fn remove_only_node() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();

        unsafe {
            list.insert(Pin::new_unchecked(&mut a));
            list.remove(Pin::new_unchecked(&mut a));
        }

        assert!(list.is_empty());
    }

    #[test]
    fn remove_clears_node_links() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let mut b = AwaiterNode::new();

        unsafe {
            list.insert(Pin::new_unchecked(&mut a));
            list.insert(Pin::new_unchecked(&mut b));
            list.remove(Pin::new_unchecked(&mut a));
        }

        assert!(a.next_in_list().is_null());
    }

    #[test]
    fn reuse_after_removal() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let mut b = AwaiterNode::new();

        b.set_user_data(2);

        unsafe {
            list.insert(Pin::new_unchecked(&mut a));
            list.remove(Pin::new_unchecked(&mut a));
            list.insert(Pin::new_unchecked(&mut b));
        }

        assert_eq!(list.take_one().unwrap().user_data(), 2);
        assert!(list.is_empty());
    }

    #[test]
    fn interleaved_push_and_pop() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let mut b = AwaiterNode::new();
        let mut c = AwaiterNode::new();

        a.set_user_data(1);
        b.set_user_data(2);
        c.set_user_data(3);

        unsafe {
            list.insert(Pin::new_unchecked(&mut a));
            list.insert(Pin::new_unchecked(&mut b));
        }

        assert_eq!(list.take_one().unwrap().user_data(), 1);

        unsafe {
            list.insert(Pin::new_unchecked(&mut c));
        }

        assert_eq!(list.take_one().unwrap().user_data(), 2);
        assert_eq!(list.take_one().unwrap().user_data(), 3);
        assert!(list.is_empty());
    }

    #[test]
    fn traversal_via_next_in_list() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let mut b = AwaiterNode::new();
        let mut c = AwaiterNode::new();

        a.set_user_data(1);
        b.set_user_data(2);
        c.set_user_data(3);

        unsafe {
            list.insert(Pin::new_unchecked(&mut a));
            list.insert(Pin::new_unchecked(&mut b));
            list.insert(Pin::new_unchecked(&mut c));
        }

        let head = list.peek().unwrap();
        assert_eq!(head.user_data(), 1);

        let second = head.next_in_list();
        assert!(!second.is_null());
        assert_eq!(unsafe { (*second).user_data() }, 2);

        let third = unsafe { (*second).next_in_list() };
        assert!(!third.is_null());
        assert_eq!(unsafe { (*third).user_data() }, 3);

        let end = unsafe { (*third).next_in_list() };
        assert!(end.is_null());
    }

    #[test]
    fn for_each_visits_all_nodes_in_order() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();
        let mut b = AwaiterNode::new();

        a.set_user_data(1);
        b.set_user_data(2);

        unsafe {
            list.insert(Pin::new_unchecked(&mut a));
            list.insert(Pin::new_unchecked(&mut b));
        }

        let mut visited = Vec::new();
        list.for_each(|node| visited.push(node.user_data()));

        assert_eq!(visited, [1, 2]);
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

        let waker = noop_waker();
        a.store_waker(waker);

        unsafe {
            list.insert(Pin::new_unchecked(&mut a));
        }

        let popped = list.take_one().unwrap();
        let recovered = popped.take_waker();
        assert!(recovered.is_some());
    }

    #[test]
    fn user_data_survives_list_operations() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();

        a.set_user_data(7);
        unsafe {
            list.insert(Pin::new_unchecked(&mut a));
        }

        let popped = list.take_one().unwrap();
        assert_eq!(popped.user_data(), 7);
    }

    #[test]
    fn notified_flag_survives_list_operations() {
        let mut list = AwaiterSet::new();
        let mut a = AwaiterNode::new();

        unsafe {
            list.insert(Pin::new_unchecked(&mut a));
        }

        let popped = list.take_one().unwrap();
        popped.set_notified();
        assert!(popped.is_notified());
    }

    #[test]
    fn ten_elements_maintain_fifo() {
        let mut list = AwaiterSet::new();
        let mut nodes: Vec<AwaiterNode> =
            std::iter::repeat_with(AwaiterNode::new).take(10).collect();

        for (i, node) in nodes.iter_mut().enumerate() {
            node.set_user_data(i);
        }

        for node in &mut nodes {
            unsafe {
                list.insert(Pin::new_unchecked(node));
            }
        }

        for i in 0..10 {
            assert_eq!(list.take_one().unwrap().user_data(), i);
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
