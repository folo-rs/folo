use std::marker::PhantomPinned;
use std::task::Waker;

// Intrusive doubly-linked list node for the waiter queue.
//
// Each node is embedded inside a wait future (behind UnsafeCell) and linked into
// the event's waiter list when the future is first polled. All field accesses go
// through raw pointers and are protected by the owning event's Mutex (sync
// variants) or by the single-threaded invariant (local variants).
pub(crate) struct WaiterNode {
    pub(crate) waker: Option<Waker>,
    pub(crate) next: *mut Self,
    pub(crate) prev: *mut Self,

    // For AutoResetEvent: set to true when this specific waiter is chosen
    // by set() to receive the token. ManualResetEvent does not use this flag.
    pub(crate) notified: bool,

    _pinned: PhantomPinned,
}

impl WaiterNode {
    pub(crate) fn new() -> Self {
        Self {
            waker: None,
            next: std::ptr::null_mut(),
            prev: std::ptr::null_mut(),
            notified: false,
            _pinned: PhantomPinned,
        }
    }
}

// FIFO doubly-linked intrusive list of waiter nodes.
//
// The list does not own the nodes — they live inside the wait futures. All
// pointer dereferences require the caller to hold the owning event's lock
// (or be on the single thread for local variants).
pub(crate) struct WaiterList {
    head: *mut WaiterNode,
    tail: *mut WaiterNode,
}

impl WaiterList {
    pub(crate) fn new() -> Self {
        Self {
            head: std::ptr::null_mut(),
            tail: std::ptr::null_mut(),
        }
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.head.is_null()
    }

    /// Appends a node to the back of the list (FIFO enqueue).
    ///
    /// # Safety
    ///
    /// * `node` must point to a valid, pinned `WaiterNode` that is not already
    ///   in any list.
    /// * The caller must hold the owning event's lock.
    pub(crate) unsafe fn push_back(&mut self, node: *mut WaiterNode) {
        let node_mut = node;

        // SAFETY: Caller guarantees node is valid and we hold the lock.
        unsafe {
            (*node_mut).next = std::ptr::null_mut();
        }

        // SAFETY: Caller guarantees node is valid and we hold the lock.
        unsafe {
            (*node_mut).prev = self.tail;
        }

        if self.tail.is_null() {
            self.head = node;
        } else {
            // SAFETY: tail is non-null and we hold the lock.
            unsafe {
                (*self.tail).next = node;
            }
        }

        self.tail = node;
    }

    /// Removes a specific node from the list.
    ///
    /// # Safety
    ///
    /// * `node` must point to a valid, pinned `WaiterNode` that is currently
    ///   in this list.
    /// * The caller must hold the owning event's lock.
    pub(crate) unsafe fn remove(&mut self, node: *mut WaiterNode) {
        // SAFETY: Caller guarantees node is valid and in the list.
        let prev = unsafe { (*node).prev };
        // SAFETY: Caller guarantees node is valid and in the list.
        let next = unsafe { (*node).next };

        if prev.is_null() {
            self.head = next;
        } else {
            // SAFETY: prev is a valid node in the list.
            unsafe {
                (*prev).next = next;
            }
        }

        if next.is_null() {
            self.tail = prev;
        } else {
            // SAFETY: next is a valid node in the list.
            unsafe {
                (*next).prev = prev;
            }
        }

        // Clear the removed node's links for safety.
        let node_mut = node;

        // SAFETY: Caller guarantees node is valid and we hold the lock.
        unsafe {
            (*node_mut).next = std::ptr::null_mut();
        }

        // SAFETY: Caller guarantees node is valid and we hold the lock.
        unsafe {
            (*node_mut).prev = std::ptr::null_mut();
        }
    }

    /// Removes and returns the front node (FIFO dequeue).
    ///
    /// # Safety
    ///
    /// * The caller must hold the owning event's lock.
    /// * Any non-null node returned is valid and pinned.
    pub(crate) unsafe fn pop_front(&mut self) -> Option<*mut WaiterNode> {
        if self.head.is_null() {
            return None;
        }

        let node = self.head;

        // SAFETY: head is non-null, so it is a valid node in the list.
        let next = unsafe { (*node).next };

        self.head = next;

        if next.is_null() {
            self.tail = std::ptr::null_mut();
        } else {
            // SAFETY: next is a valid node in the list.
            unsafe {
                (*next).prev = std::ptr::null_mut();
            }
        }

        // Clear the removed node's links.
        // SAFETY: node is valid (we just read from it above).
        unsafe {
            (*node).next = std::ptr::null_mut();
        }

        // SAFETY: node is valid.
        unsafe {
            (*node).prev = std::ptr::null_mut();
        }

        Some(node)
    }

    /// Iterates over all nodes, calling `f` with each node pointer.
    /// Nodes are visited front-to-back. The callback must not modify
    /// the list structure.
    ///
    /// # Safety
    ///
    /// * The caller must hold the owning event's lock.
    /// * The callback must not add or remove nodes from the list.
    pub(crate) unsafe fn for_each(&self, mut f: impl FnMut(*mut WaiterNode)) {
        let mut current = self.head;
        while !current.is_null() {
            // Read next before calling f, in case f invalidates current
            // (it should not per the contract, but this is defensive).
            // SAFETY: current is non-null and in the list.
            let next = unsafe { (*current).next };
            f(current);
            current = next;
        }
    }
}

// SAFETY: WaiterList contains raw pointers (*mut WaiterNode), which are !Send
// by default. Sending a WaiterList to another thread is safe because the
// pointers are never dereferenced without holding the owning event's Mutex,
// ensuring exclusive access at any point in time.
unsafe impl Send for WaiterList {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block,
    clippy::indexing_slicing,
    reason = "test-only code with trivial safety invariants"
)]
mod tests {
    use super::*;

    #[test]
    fn push_back_and_pop_front_fifo_order() {
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
    fn for_each_visits_all_nodes() {
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
}
