use std::panic::{RefUnwindSafe, UnwindSafe};
use std::ptr;
use std::task::Waker;

use crate::awaiter::Awaiter;

/// A set of pinned [`Awaiter`]s awaiting notification.
///
/// The set does not own its nodes. Nodes are embedded in async futures
/// and linked/unlinked as awaiters register and complete.
///
/// The set is `Send` but not `Sync` — it can be moved between threads
/// but all access must be serialized by the caller.
pub struct AwaiterSet {
    head: *mut Awaiter,
    tail: *mut Awaiter,
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

    /// Returns `true` if the set contains no awaiters.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.head.is_null()
    }

    /// Returns a shared reference to the next awaiter to be taken.
    ///
    /// Returns `None` if the set is empty.
    #[must_use]
    pub fn peek(&self) -> Option<&Awaiter> {
        if self.head.is_null() {
            None
        } else {
            // SAFETY: All pointers in the set are valid and pinned,
            // an invariant established by `insert`.
            Some(unsafe { &*self.head })
        }
    }

    /// Inserts an awaiter into the set.
    ///
    /// # Safety
    ///
    /// `ptr` must point to a valid, pinned [`Awaiter`] that will
    /// remain valid until removed from the set.
    pub(crate) unsafe fn insert(&mut self, ptr: *mut Awaiter) {
        // SAFETY: Caller guarantees `ptr` is valid.
        let node = unsafe { &mut *ptr };
        node.set_next(ptr::null_mut());
        node.set_prev(self.tail);

        if self.tail.is_null() {
            self.head = ptr;
        } else {
            // SAFETY: `tail` is non-null and valid (set invariant).
            unsafe {
                (*self.tail).set_next(ptr);
            }
        }

        self.tail = ptr;
    }

    /// Removes a specific awaiter from the set.
    ///
    /// # Safety
    ///
    /// `ptr` must point to a valid [`Awaiter`] currently in this set.
    pub(crate) unsafe fn remove(&mut self, ptr: *mut Awaiter) {
        // SAFETY: Caller guarantees `ptr` is valid and in the set.
        let node = unsafe { &*ptr };
        let prev = node.prev();
        let next = node.next();

        if prev.is_null() {
            self.head = next;
        } else {
            // SAFETY: `prev` is a valid awaiter in the set.
            unsafe {
                (*prev).set_next(next);
            }
        }

        if next.is_null() {
            self.tail = prev;
        } else {
            // SAFETY: `next` is a valid awaiter in the set.
            unsafe {
                (*next).set_prev(prev);
            }
        }

        // Clear the removed node's links.
        // SAFETY: `ptr` is valid (caller contract).
        let node = unsafe { &mut *ptr };
        node.set_next(ptr::null_mut());
        node.set_prev(ptr::null_mut());
    }

    /// Removes one awaiter from the set and notifies it.
    ///
    /// Transitions the awaiter from Waiting to Notified and returns
    /// its stored waker. The caller should invoke
    /// [`Waker::wake()`] outside any lock scope to avoid re-entrancy
    /// issues.
    ///
    /// Returns `None` if the set is empty.
    pub fn notify_one(&mut self) -> Option<Waker> {
        let awaiter = self.take_one()?;
        awaiter.notify()
    }

    // Removes and returns one awaiter from the set.
    fn take_one(&mut self) -> Option<&mut Awaiter> {
        if self.head.is_null() {
            return None;
        }

        let ptr = self.head;

        // SAFETY: `head` is non-null, so it is a valid awaiter.
        let next = unsafe { (*ptr).next() };

        self.head = next;

        if next.is_null() {
            self.tail = ptr::null_mut();
        } else {
            // SAFETY: `next` is a valid awaiter in the set.
            unsafe {
                (*next).set_prev(ptr::null_mut());
            }
        }

        // Clear the removed awaiter's links.
        // SAFETY: `ptr` is valid (we just read from it above).
        let node = unsafe { &mut *ptr };
        node.set_next(ptr::null_mut());
        node.set_prev(ptr::null_mut());

        // We just removed it, so no other reference exists in the
        // set. The `&mut self` borrow prevents concurrent operations.
        Some(node)
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
    use std::pin::Pin;
    use std::task::Waker;

    use super::*;

    static_assertions::assert_impl_all!(AwaiterSet: Send, UnwindSafe, RefUnwindSafe);
    static_assertions::assert_not_impl_any!(AwaiterSet: Sync);

    fn waker() -> Waker {
        Waker::noop().clone()
    }

    #[test]
    fn new_list_is_empty() {
        let list = AwaiterSet::new();
        assert!(list.is_empty());
        assert!(list.peek().is_none());
    }

    #[test]
    fn default_list_is_empty() {
        let list = AwaiterSet::default();
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
        let mut a = Awaiter::new();

        unsafe {
            Pin::new_unchecked(&mut a).register_with_data(&mut list, waker(), 1);
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
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();
        let mut c = Awaiter::new();

        unsafe {
            Pin::new_unchecked(&mut a).register_with_data(&mut list, waker(), 1);
            Pin::new_unchecked(&mut b).register_with_data(&mut list, waker(), 2);
            Pin::new_unchecked(&mut c).register_with_data(&mut list, waker(), 3);
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
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();

        unsafe {
            Pin::new_unchecked(&mut a).register_with_data(&mut list, waker(), 1);
            Pin::new_unchecked(&mut b).register_with_data(&mut list, waker(), 2);
            Pin::new_unchecked(&mut a).unregister(&mut list);
        }

        assert_eq!(list.take_one().unwrap().user_data(), 2);
        assert!(list.is_empty());
    }

    #[test]
    fn remove_tail_node() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();

        unsafe {
            Pin::new_unchecked(&mut a).register_with_data(&mut list, waker(), 1);
            Pin::new_unchecked(&mut b).register_with_data(&mut list, waker(), 2);
            Pin::new_unchecked(&mut b).unregister(&mut list);
        }

        assert_eq!(list.take_one().unwrap().user_data(), 1);
        assert!(list.is_empty());
    }

    #[test]
    fn remove_middle_node() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();
        let mut c = Awaiter::new();

        unsafe {
            Pin::new_unchecked(&mut a).register_with_data(&mut list, waker(), 1);
            Pin::new_unchecked(&mut b).register_with_data(&mut list, waker(), 2);
            Pin::new_unchecked(&mut c).register_with_data(&mut list, waker(), 3);
            Pin::new_unchecked(&mut b).unregister(&mut list);
        }

        assert_eq!(list.take_one().unwrap().user_data(), 1);
        assert_eq!(list.take_one().unwrap().user_data(), 3);
        assert!(list.is_empty());
    }

    #[test]
    fn remove_only_node() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            Pin::new_unchecked(&mut a).register(&mut list, waker());
            Pin::new_unchecked(&mut a).unregister(&mut list);
        }

        assert!(list.is_empty());
    }

    #[test]
    fn remove_clears_node_links() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();

        unsafe {
            Pin::new_unchecked(&mut a).register(&mut list, waker());
            Pin::new_unchecked(&mut b).register(&mut list, waker());
            Pin::new_unchecked(&mut a).unregister(&mut list);
        }

        assert!(a.next().is_null());
    }

    #[test]
    fn reuse_after_removal() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();

        unsafe {
            Pin::new_unchecked(&mut a).register(&mut list, waker());
            Pin::new_unchecked(&mut a).unregister(&mut list);
            Pin::new_unchecked(&mut b).register_with_data(&mut list, waker(), 2);
        }

        assert_eq!(list.take_one().unwrap().user_data(), 2);
        assert!(list.is_empty());
    }

    #[test]
    fn interleaved_push_and_pop() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();
        let mut c = Awaiter::new();

        unsafe {
            Pin::new_unchecked(&mut a).register_with_data(&mut list, waker(), 1);
            Pin::new_unchecked(&mut b).register_with_data(&mut list, waker(), 2);
        }

        assert_eq!(list.take_one().unwrap().user_data(), 1);

        unsafe {
            Pin::new_unchecked(&mut c).register_with_data(&mut list, waker(), 3);
        }

        assert_eq!(list.take_one().unwrap().user_data(), 2);
        assert_eq!(list.take_one().unwrap().user_data(), 3);
        assert!(list.is_empty());
    }

    #[test]
    fn traversal_via_next() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();
        let mut c = Awaiter::new();

        unsafe {
            Pin::new_unchecked(&mut a).register_with_data(&mut list, waker(), 1);
            Pin::new_unchecked(&mut b).register_with_data(&mut list, waker(), 2);
            Pin::new_unchecked(&mut c).register_with_data(&mut list, waker(), 3);
        }

        let head = list.peek().unwrap();
        assert_eq!(head.user_data(), 1);

        let second = head.next();
        assert!(!second.is_null());
        assert_eq!(unsafe { (*second).user_data() }, 2);

        let third = unsafe { (*second).next() };
        assert!(!third.is_null());
        assert_eq!(unsafe { (*third).user_data() }, 3);

        let end = unsafe { (*third).next() };
        assert!(end.is_null());
    }

    #[test]
    fn wakers_survive_list_operations() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            Pin::new_unchecked(&mut a).register(&mut list, waker());
        }

        let popped = list.take_one().unwrap();
        let recovered = popped.take_waker();
        assert!(recovered.is_some());
    }

    #[test]
    fn user_data_survives_list_operations() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            Pin::new_unchecked(&mut a).register_with_data(&mut list, waker(), 7);
        }

        let popped = list.take_one().unwrap();
        assert_eq!(popped.user_data(), 7);
    }

    #[test]
    fn notified_flag_survives_list_operations() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            Pin::new_unchecked(&mut a).register(&mut list, waker());
        }

        drop(list.notify_one());
        // SAFETY: The awaiter is not moved.
        assert!(unsafe { Pin::new_unchecked(&a).is_notified() });
    }

    #[test]
    fn ten_elements_maintain_fifo() {
        let mut list = AwaiterSet::new();
        let mut nodes: Vec<Awaiter> = std::iter::repeat_with(Awaiter::new).take(10).collect();

        for (i, node) in nodes.iter_mut().enumerate() {
            unsafe {
                Pin::new_unchecked(node).register_with_data(&mut list, waker(), i);
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
