use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomPinned;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::task::Waker;

use crate::{WaiterList, WaiterNode};

/// A slot for embedding a [`WaiterNode`] inside a wait future.
///
/// Wraps an [`UnsafeCell<WaiterNode>`] together with a registration flag and a
/// pinning marker — the three fields that every wait future needs. The slot
/// provides safe accessors where possible and consolidates the common
/// register/unregister patterns behind a smaller number of `unsafe` calls.
///
/// # Usage
///
/// Embed a `WaiterSlot` in your future struct instead of maintaining separate
/// `UnsafeCell<WaiterNode>`, `registered: bool`, and `PhantomPinned` fields:
///
/// ```ignore
/// struct MyWaitFuture {
///     slot: WaiterSlot,
///     // ... other future-specific fields
/// }
/// ```
///
/// # Safety model
///
/// Methods that read or write the inner [`WaiterNode`] are `unsafe` because
/// the caller must guarantee exclusive access — either by holding a mutex or
/// by confining all access to a single thread (`!Send`).
///
/// Two methods are safe:
///
/// * [`is_registered()`][Self::is_registered] reads a plain `bool` owned
///   by the slot.
/// * [`node_ptr()`][Self::node_ptr] returns a raw pointer without
///   dereferencing it.
pub struct WaiterSlot {
    node: UnsafeCell<WaiterNode>,
    registered: bool,
    _pinned: PhantomPinned,
}

impl WaiterSlot {
    /// Creates a new slot with an unlinked, unregistered node.
    #[must_use]
    pub fn new() -> Self {
        Self {
            node: UnsafeCell::new(WaiterNode::new()),
            registered: false,
            _pinned: PhantomPinned,
        }
    }

    /// Returns `true` if the node is currently registered in a
    /// [`WaiterList`].
    #[must_use]
    pub fn is_registered(&self) -> bool {
        self.registered
    }

    /// Returns a raw pointer to the inner [`WaiterNode`].
    ///
    /// Obtaining the pointer is safe. Dereferencing it requires the
    /// caller to guarantee exclusive access to the node (e.g. by
    /// holding a lock or confining access to one thread).
    #[must_use]
    pub fn node_ptr(&self) -> *mut WaiterNode {
        self.node.get()
    }

    /// Stores a waker and registers the node in `list` if not already
    /// registered.
    ///
    /// On the first call this pushes the node into the list and marks
    /// the slot as registered. On subsequent calls it only updates the
    /// stored waker (the node is already in the list).
    ///
    /// Takes ownership of the waker so no cloning happens while the
    /// caller holds a lock. Clone the waker before acquiring locks.
    ///
    /// # Safety
    ///
    /// * The caller must have exclusive access to both the node and
    ///   the list (e.g. by holding a lock).
    /// * The slot must be at a pinned, stable address.
    pub unsafe fn register(&mut self, list: &mut WaiterList, waker: Waker) {
        let node_ptr = self.node.get();
        // SAFETY: Caller guarantees exclusive access.
        unsafe {
            (*node_ptr).store_waker(waker);
        }
        if !self.registered {
            // SAFETY: Caller guarantees exclusive access and the
            // node is pinned and not in any list.
            unsafe {
                list.push_back(node_ptr);
            }
            self.registered = true;
        }
    }

    /// Stores a waker with caller-defined data and registers the node
    /// in `list` if not already registered.
    ///
    /// Behaves like [`register()`][Self::register] but also sets the
    /// node's [`user_data`][WaiterNode::user_data] (e.g. the number
    /// of permits a semaphore waiter requests).
    ///
    /// # Safety
    ///
    /// Same requirements as [`register()`][Self::register].
    pub unsafe fn register_with_data(&mut self, list: &mut WaiterList, waker: Waker, data: usize) {
        let node_ptr = self.node.get();
        // SAFETY: Caller guarantees exclusive access.
        unsafe {
            (*node_ptr).store_waker(waker);
        }
        // SAFETY: Caller guarantees exclusive access.
        unsafe {
            (*node_ptr).set_user_data(data);
        }
        if !self.registered {
            // SAFETY: Caller guarantees exclusive access and the
            // node is pinned and not in any list.
            unsafe {
                list.push_back(node_ptr);
            }
            self.registered = true;
        }
    }

    /// Removes the node from `list` if it is currently registered.
    ///
    /// After this call, [`is_registered()`][Self::is_registered]
    /// returns `false`.
    ///
    /// # Safety
    ///
    /// * The caller must have exclusive access to both the node and
    ///   the list.
    /// * The node must be in `list` (not some other list).
    pub unsafe fn unregister(&mut self, list: &mut WaiterList) {
        if self.registered {
            // SAFETY: Caller guarantees exclusive access and that
            // the node is in this list.
            unsafe {
                list.remove(self.node.get());
            }
            self.registered = false;
        }
    }

    /// Checks whether the node was notified and, if so, clears the
    /// registration flag.
    ///
    /// Returns `true` if the node's notified flag is set, meaning
    /// the synchronization primitive has popped this node from the
    /// list and transferred ownership of some resource (a lock, a
    /// permit, or a signal) to it. The slot is then marked as
    /// unregistered because the primitive already removed the node.
    ///
    /// This is the standard first step in every `poll()` method:
    ///
    /// ```ignore
    /// // SAFETY: We hold the lock.
    /// if unsafe { slot.take_notification() } {
    ///     return Poll::Ready(());
    /// }
    /// ```
    ///
    /// # Safety
    ///
    /// * The caller must have exclusive access to the node.
    pub unsafe fn take_notification(&mut self) -> bool {
        let node_ptr = self.node.get();
        // SAFETY: Caller guarantees exclusive access.
        if unsafe { (*node_ptr).is_notified() } {
            self.registered = false;
            true
        } else {
            false
        }
    }

    /// Checks whether the node was notified, without changing
    /// registration state.
    ///
    /// Use this in drop handlers to decide whether to forward a
    /// resource to the next waiter.
    ///
    /// # Safety
    ///
    /// * The caller must have exclusive access to the node.
    pub unsafe fn is_notified(&self) -> bool {
        let node_ptr = self.node.get();
        // SAFETY: Caller guarantees exclusive access.
        unsafe { (*node_ptr).is_notified() }
    }
}

impl Default for WaiterSlot {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: WaiterSlot is used in futures that are `Send`. The raw
// pointers inside WaiterNode are only dereferenced while the caller
// holds a lock, so sending across threads is safe.
unsafe impl Send for WaiterSlot {}

// WaiterSlot is !Sync because it contains UnsafeCell and raw pointers.
// The UnsafeCell<WaiterNode> already makes the type !Sync via auto
// trait rules, so no explicit marker is needed.

// The slot contains no interior mutability visible to callers (all
// mutation requires unsafe + exclusive access). Observing inconsistent
// state during unwind is not possible.
impl UnwindSafe for WaiterSlot {}
impl RefUnwindSafe for WaiterSlot {}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for WaiterSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WaiterSlot")
            .field("registered", &self.registered)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::task::Waker;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(WaiterSlot: Send, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(WaiterSlot: Sync);

    #[test]
    fn new_slot_is_unregistered() {
        let slot = WaiterSlot::new();
        assert!(!slot.is_registered());
    }

    #[test]
    fn default_slot_is_unregistered() {
        let slot = WaiterSlot::default();
        assert!(!slot.is_registered());
    }

    #[test]
    fn node_ptr_is_stable() {
        let slot = WaiterSlot::new();
        let p1 = slot.node_ptr();
        let p2 = slot.node_ptr();
        assert_eq!(p1, p2);
        assert!(!p1.is_null());
    }

    #[test]
    fn register_sets_registered_flag() {
        let mut slot = WaiterSlot::new();
        let mut list = WaiterList::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            slot.register(&mut list, Waker::noop().clone());
        }
        assert!(slot.is_registered());
        assert!(!list.is_empty());
    }

    #[test]
    fn register_idempotent_on_second_call() {
        let mut slot = WaiterSlot::new();
        let mut list = WaiterList::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            slot.register(&mut list, Waker::noop().clone());
        }
        // SAFETY: Test has exclusive access.
        unsafe {
            slot.register(&mut list, Waker::noop().clone());
        }

        // Still registered, list still has exactly one node.
        assert!(slot.is_registered());
        // SAFETY: Test has exclusive access.
        let popped = unsafe { list.pop_front() };
        assert!(popped.is_some());
        assert!(list.is_empty());
    }

    #[test]
    fn register_with_data_stores_user_data() {
        let mut slot = WaiterSlot::new();
        let mut list = WaiterList::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            slot.register_with_data(&mut list, Waker::noop().clone(), 42);
        }
        assert!(slot.is_registered());

        // SAFETY: Test has exclusive access.
        let data = unsafe { (*slot.node_ptr()).user_data() };
        assert_eq!(data, 42);
    }

    #[test]
    fn unregister_removes_from_list() {
        let mut slot = WaiterSlot::new();
        let mut list = WaiterList::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            slot.register(&mut list, Waker::noop().clone());
        }
        // SAFETY: Test has exclusive access.
        unsafe {
            slot.unregister(&mut list);
        }

        assert!(!slot.is_registered());
        assert!(list.is_empty());
    }

    #[test]
    fn unregister_when_not_registered_is_noop() {
        let mut slot = WaiterSlot::new();
        let mut list = WaiterList::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            slot.unregister(&mut list);
        }
        assert!(!slot.is_registered());
    }

    #[test]
    fn take_notification_returns_false_when_not_notified() {
        let mut slot = WaiterSlot::new();

        // SAFETY: Test has exclusive access.
        let notified = unsafe { slot.take_notification() };
        assert!(!notified);
    }

    #[test]
    fn take_notification_returns_true_and_clears_registered() {
        let mut slot = WaiterSlot::new();
        let mut list = WaiterList::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            slot.register(&mut list, Waker::noop().clone());
        }

        // Simulate what the primitive does: pop + set_notified.
        // SAFETY: Test has exclusive access.
        let node = unsafe { list.pop_front() }.unwrap();
        // SAFETY: Test has exclusive access, node is valid.
        unsafe {
            (*node).set_notified();
        }

        // The slot still thinks it is registered (the primitive
        // popped it, but the slot does not know yet).
        assert!(slot.is_registered());

        // SAFETY: Test has exclusive access.
        let notified = unsafe { slot.take_notification() };
        assert!(notified);
        assert!(!slot.is_registered());
    }

    #[test]
    fn is_notified_does_not_change_registered() {
        let mut slot = WaiterSlot::new();
        let mut list = WaiterList::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            slot.register(&mut list, Waker::noop().clone());
        }
        // SAFETY: Test has exclusive access.
        let node = unsafe { list.pop_front() }.unwrap();
        // SAFETY: Test has exclusive access, node is valid.
        unsafe {
            (*node).set_notified();
        }

        // SAFETY: Test has exclusive access.
        let notified = unsafe { slot.is_notified() };
        assert!(notified);
        // registered is unchanged.
        assert!(slot.is_registered());
    }

    #[test]
    fn full_lifecycle_register_notify_take() {
        let mut slot = WaiterSlot::new();
        let mut list = WaiterList::new();

        // Register.
        // SAFETY: Test has exclusive access.
        unsafe {
            slot.register(&mut list, Waker::noop().clone());
        }
        assert!(slot.is_registered());

        // Simulate notification.
        // SAFETY: Test has exclusive access.
        let node = unsafe { list.pop_front() }.unwrap();
        // SAFETY: Test has exclusive access, node is valid.
        unsafe {
            (*node).set_notified();
        }

        // Take notification (as poll would do).
        // SAFETY: Test has exclusive access.
        let notified = unsafe { slot.take_notification() };
        assert!(notified);
        assert!(!slot.is_registered());
    }

    #[test]
    fn full_lifecycle_register_unregister() {
        let mut slot = WaiterSlot::new();
        let mut list = WaiterList::new();

        // Register.
        // SAFETY: Test has exclusive access.
        unsafe {
            slot.register(&mut list, Waker::noop().clone());
        }
        assert!(slot.is_registered());

        // Unregister (as drop of a non-notified future would do).
        // SAFETY: Test has exclusive access.
        unsafe {
            slot.unregister(&mut list);
        }
        assert!(!slot.is_registered());
        assert!(list.is_empty());
    }
}
