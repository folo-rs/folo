use std::marker::PhantomPinned;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::task::Waker;

/// A node in an intrusive doubly-linked [`crate::WaiterList`].
///
/// Each node stores a [`Waker`] for async notification, a boolean notification
/// flag, and a `usize` of caller-defined data. Nodes are embedded directly
/// inside wait futures and linked into a [`crate::WaiterList`] via raw pointers.
///
/// Once a node has been pushed into a list, it must remain at a stable memory
/// address until it is removed. The [`PhantomPinned`] marker enforces this
/// contract at the type level — the containing future must be pinned before
/// the node can be registered.
///
/// # Fields
///
/// All fields are private and accessed through methods:
///
/// * **Waker** — stored by the future's `poll()`, consumed by the primitive's
///   notification logic. See [`store_waker()`][Self::store_waker] and
///   [`take_waker()`][Self::take_waker].
/// * **Notified flag** — set by the primitive after popping the node, checked
///   by the future on its next poll. See [`is_notified()`][Self::is_notified]
///   and [`set_notified()`][Self::set_notified].
/// * **User data** — a `usize` of caller-defined metadata (e.g. the number
///   of permits a semaphore waiter requests). Defaults to `0`. See
///   [`user_data()`][Self::user_data] and
///   [`set_user_data()`][Self::set_user_data].
#[non_exhaustive]
pub struct WaiterNode {
    /// The waker to call when this waiter is selected for notification.
    waker: Option<Waker>,

    /// Set to `true` after this node is popped from the list by the
    /// synchronization primitive. The owning future checks this flag
    /// on the next poll to complete with `Ready`.
    notified: bool,

    /// Caller-defined metadata. Semaphores store the requested permit
    /// count here; other primitives leave it at the default of `0`.
    user_data: usize,

    /// Intrusive doubly-linked list pointers, managed by [`WaiterList`].
    pub(crate) next: *mut Self,
    pub(crate) prev: *mut Self,

    _pinned: PhantomPinned,
}

impl WaiterNode {
    /// Creates a new unlinked node.
    ///
    /// The node starts with no waker, `notified` set to `false`,
    /// `user_data` set to `0`, and null list pointers.
    ///
    /// # Examples
    ///
    /// ```
    /// use waiter_list::WaiterNode;
    ///
    /// let node = WaiterNode::new();
    /// assert!(!node.is_notified());
    /// assert_eq!(node.user_data(), 0);
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            waker: None,
            next: std::ptr::null_mut(),
            prev: std::ptr::null_mut(),
            notified: false,
            user_data: 0,
            _pinned: PhantomPinned,
        }
    }

    /// Stores a waker for this node, replacing any previously stored waker.
    ///
    /// Takes ownership of the waker to avoid cloning under a lock.
    /// Callers should clone the waker before acquiring any locks and
    /// pass the owned clone here.
    pub fn store_waker(&mut self, waker: Waker) {
        self.waker = Some(waker);
    }

    /// Extracts and returns the stored waker, if any.
    ///
    /// After this call, the node holds no waker. The caller is responsible
    /// for calling [`Waker::wake()`] outside any lock scope to avoid
    /// re-entrancy issues.
    pub fn take_waker(&mut self) -> Option<Waker> {
        self.waker.take()
    }

    /// Returns whether this node has been marked as notified.
    #[must_use]
    pub fn is_notified(&self) -> bool {
        self.notified
    }

    /// Marks this node as notified.
    ///
    /// Synchronization primitives call this after popping the node from
    /// the list, signaling the owning future to complete on its next poll.
    pub fn set_notified(&mut self) {
        self.notified = true;
    }

    /// Returns the caller-defined user data associated with this node.
    ///
    /// Defaults to `0` if never set.
    #[must_use]
    pub fn user_data(&self) -> usize {
        self.user_data
    }

    /// Sets the caller-defined user data for this node.
    ///
    /// Semaphores use this to store the number of permits the waiter
    /// requests; other primitives may ignore it.
    pub fn set_user_data(&mut self, data: usize) {
        self.user_data = data;
    }

    /// Returns a raw pointer to the next node in the list.
    ///
    /// Returns a null pointer if this is the last node or the node is
    /// not currently in a list. The returned pointer is valid only as
    /// long as the caller maintains exclusive access to the list.
    #[must_use]
    pub fn next_in_list(&self) -> *mut Self {
        self.next
    }
}

impl Default for WaiterNode {
    fn default() -> Self {
        Self::new()
    }
}

// WaiterNode contains raw pointers (*mut Self) which make it !Send and !Sync
// by default. This is correct: nodes are pinned at a fixed address and must
// not be moved across threads. Consumers wrap nodes in UnsafeCell and handle
// Send via their own unsafe impls on the containing future type.

impl UnwindSafe for WaiterNode {}
impl RefUnwindSafe for WaiterNode {}

impl std::fmt::Debug for WaiterNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WaiterNode")
            .field("has_waker", &self.waker.is_some())
            .field("notified", &self.notified)
            .field("user_data", &self.user_data)
            .field("next", &self.next)
            .field("prev", &self.prev)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::undocumented_unsafe_blocks,
    reason = "test code with trivial safety invariants"
)]
mod tests {
    use std::task::{RawWaker, RawWakerVTable, Waker};

    use super::*;

    static_assertions::assert_not_impl_any!(WaiterNode: Send, Sync);
    static_assertions::assert_impl_all!(WaiterNode: UnwindSafe, RefUnwindSafe);

    fn noop_waker() -> Waker {
        fn clone(data: *const ()) -> RawWaker {
            RawWaker::new(data, &VTABLE)
        }
        fn noop(_: *const ()) {}
        static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
        // SAFETY: The vtable functions are valid no-ops.
        unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
    }

    #[test]
    fn new_node_has_expected_defaults() {
        let mut node = WaiterNode::new();
        assert!(!node.is_notified());
        assert_eq!(node.user_data(), 0);
        assert!(node.next_in_list().is_null());
        assert!(node.take_waker().is_none());
    }

    #[test]
    fn default_is_same_as_new() {
        let node = WaiterNode::default();
        assert!(!node.is_notified());
        assert_eq!(node.user_data(), 0);
    }

    #[test]
    fn store_and_take_waker() {
        let mut node = WaiterNode::new();
        let waker = noop_waker();
        node.store_waker(waker);
        assert!(node.take_waker().is_some());
    }

    #[test]
    fn take_waker_twice_returns_none() {
        let mut node = WaiterNode::new();
        let waker = noop_waker();
        node.store_waker(waker);
        drop(node.take_waker());
        assert!(node.take_waker().is_none());
    }

    #[test]
    fn store_waker_replaces_previous() {
        let mut node = WaiterNode::new();
        let w1 = noop_waker();
        let w2 = noop_waker();
        node.store_waker(w1);
        node.store_waker(w2);
        assert!(node.take_waker().is_some());
    }

    #[test]
    fn set_and_check_notified() {
        let mut node = WaiterNode::new();
        assert!(!node.is_notified());
        node.set_notified();
        assert!(node.is_notified());
    }

    #[test]
    fn set_and_get_user_data() {
        let mut node = WaiterNode::new();
        node.set_user_data(42);
        assert_eq!(node.user_data(), 42);
    }

    #[test]
    fn debug_output_does_not_panic() {
        let node = WaiterNode::new();
        let debug = format!("{node:?}");
        assert!(debug.contains("WaiterNode"));
    }
}
