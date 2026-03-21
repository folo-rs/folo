// Intrusive doubly-linked waiter queue shared by all event types.
//
// # Design
//
// This module provides a FIFO doubly-linked list of waiter nodes used to
// park and wake futures that are waiting on an event. The list is intrusive:
// each node is embedded directly inside the wait future (behind an
// `UnsafeCell`) rather than being heap-allocated separately.
//
// # Ownership model
//
// The `WaiterList` does not own its nodes. Each `WaiterNode` is a field of
// the wait future that created it (e.g. `AutoResetWaitFuture`). The node is
// linked into the list when the future is first polled and unlinked either
// by the future's `Drop` impl or (for auto-reset events) by `set()`.
//
// Nodes are pinned (`PhantomPinned`) because the list holds raw pointers to
// them. The containing future must be pinned before polling, ensuring the
// node address remains stable for its entire lifetime in the list.
//
// # Thread safety
//
// The list itself has no internal synchronization. Callers are responsible
// for ensuring exclusive access:
//
// * **Sync event variants** (`ManualResetEvent`, `AutoResetEvent`): all node
//   and list access is protected by the owning event's `Mutex`.
// * **Local event variants** (`LocalManualResetEvent`, `LocalAutoResetEvent`):
//   the containing types are `!Send`, so all access is confined to a single
//   thread. No lock is needed.
//
// `WaiterList` has an explicit `unsafe impl Send` because it contains raw
// pointers (`*mut WaiterNode`), which are `!Send` by default in Rust.
// Sending is safe because sync variants wrap the list in a `Mutex` (so
// pointers are never dereferenced without the lock), and local variants
// never actually send the list across threads (their container is `!Send`).
//
// # Node lifecycle
//
// 1. **Creation**: a `WaiterNode` is created (unlinked, `notified = false`,
//    `waker = None`) when the wait future is constructed via `event.wait()`.
//
// 2. **Registration**: on the first `poll()`, the future stores the caller's
//    waker in the node and pushes it onto the list via `push_back()`. The
//    future's `registered` flag is set to `true`.
//
// 3. **Re-poll**: if polled again before being woken, the future replaces
//    the stored waker with the (possibly new) one from the context. The
//    node stays in the list.
//
// 4. **Notification**: `set()` wakes one or more nodes depending on the
//    event type:
//    - **Auto-reset**: `pop_front()` removes one node, sets its `notified`
//      flag to `true`, takes the waker, and calls `wake()`. The node is no
//      longer in the list.
//    - **Manual-reset**: walks the list via `head()`, takes each node's
//      waker, and calls `wake()` — but nodes remain in the list. Futures
//      remove themselves on their next poll (which sees `is_set == true`).
//
// 5. **Completion**: when the woken future is polled again, it sees either
//    `notified == true` (auto-reset) or `is_set == true` (manual-reset),
//    unlinks itself if still registered, and returns `Poll::Ready`.
//
// 6. **Cancellation (Drop)**: if a future is dropped while registered:
//    - **Auto-reset, not notified**: simply removes the node from the list.
//    - **Auto-reset, notified**: the signal must not be lost, so the drop
//      impl forwards the notification to the next waiter (or re-sets the
//      `is_set` flag if no waiters remain).
//    - **Manual-reset**: simply removes the node from the list. No signal
//      forwarding is needed because `is_set` remains `true`.
//
// # The `notified` flag
//
// Used only by auto-reset events. When `set()` pops a node, it sets
// `notified = true` on that node before waking. This serves two purposes:
//
// * The future's `poll()` can detect that it was specifically chosen by
//   `set()` and should return `Ready`, even though the event's `is_set`
//   flag may already be `false` (consumed by another waiter).
// * The future's `Drop` knows to forward the signal if the future is
//   cancelled after being notified but before being polled to completion.
//
// # The `registered` flag
//
// Tracked on the future (not the node) to know whether the node is
// currently in the list. This avoids scanning the list to check membership
// and allows `Drop` to skip cleanup when the future was never polled or
// has already been completed.
//
// # UnsafeCell for the node
//
// The node is wrapped in `UnsafeCell` inside the future because both
// `poll()` (via `Pin::get_unchecked_mut`) and the event's `set()` (via
// raw pointers from the list) need to access the same node. `UnsafeCell`
// opts out of Rust's aliasing guarantees, making this sound as long as
// accesses are serialized — which they are, by the Mutex or single-thread
// invariant described above.
//
// # Re-entrancy in `set()`
//
// Waker `wake()` calls can be re-entrant: a waker may immediately poll
// the future or drop it, causing further list mutations. The `set()`
// implementations handle this by:
//
// * **Sync variants**: releasing the Mutex before calling `wake()`, then
//   re-acquiring it and rescanning from the list head. No node pointer
//   survives across the lock release.
// * **Local variants**: accessing the list only through raw pointers (not
//   `&mut WaiterList` references) and rescanning from head after each
//   wake. No reference or stored `next` pointer survives across `wake()`.

use std::marker::PhantomPinned;
use std::task::Waker;

pub(crate) struct WaiterNode {
    pub(crate) waker: Option<Waker>,
    pub(crate) next: *mut Self,
    pub(crate) prev: *mut Self,
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
    /// * The caller must ensure exclusive access to the list.
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
    /// * The caller must ensure exclusive access to the list.
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
    /// * The caller must ensure exclusive access to the list.
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
    /// * The caller must ensure exclusive access to the list.
    /// * The callback must not add or remove nodes from the list.
    #[cfg(test)]
    pub(crate) unsafe fn for_each(&self, mut f: impl FnMut(*mut WaiterNode)) {
        let mut current = self.head;
        while !current.is_null() {
            // Read next before calling f so we do not hold a reference to
            // the current node across the callback.
            // SAFETY: current is non-null and in the list.
            let next = unsafe { (*current).next };
            f(current);
            current = next;
        }
    }

    /// Returns a pointer to the head node, or null if the list is empty.
    pub(crate) fn head(&self) -> *mut WaiterNode {
        self.head
    }
}

// SAFETY: See the module-level comment for the full thread safety rationale.
// Raw pointers are `!Send` by default; this impl is sound because all pointer
// dereferences are serialized by external synchronization (Mutex or
// single-thread confinement).
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
