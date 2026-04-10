use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::ptr;
use std::task::Waker;

use crate::awaiter::{Awaiter, State};

/// Tracks awaiters for a synchronization primitive.
///
/// Owned by the primitive (e.g. a mutex or event). Asynchronous
/// futures that cannot complete immediately
/// [`register()`][Self::register] an [`Awaiter`] with this set.
/// When the primitive grants a resource, it calls
/// [`notify_one()`][Self::notify_one] to remove an awaiter and
/// return its [`Waker`].
///
/// # Synchronization
///
/// The set has no internal synchronization. The primitive must
/// serialize all access to both the set and every [`Awaiter`]
/// registered with it — for example, by protecting them with a
/// single [`Mutex`][std::sync::Mutex] or by confining the
/// containing type to a single thread.
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
    /// let set = AwaiterSet::new();
    /// // SAFETY: No concurrent access in this example.
    /// assert!(unsafe { set.is_empty() });
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            head: ptr::null_mut(),
            tail: ptr::null_mut(),
        }
    }

    /// Returns `true` if the set contains no awaiters.
    ///
    /// # Safety
    ///
    /// The set and all its awaiters must be protected by the same
    /// lock (or confined to a single thread).
    #[must_use]
    pub unsafe fn is_empty(&self) -> bool {
        self.head.is_null()
    }

    /// Returns a shared reference to an awaiter in the set.
    ///
    /// Useful for inspecting awaiter metadata (e.g. checking
    /// [`user_data()`][Awaiter::user_data] before deciding whether
    /// to notify). Returns `None` if the set is empty.
    ///
    /// # Safety
    ///
    /// The set and all its awaiters must be protected by the same
    /// lock (or confined to a single thread).
    #[must_use]
    pub unsafe fn peek(&self) -> Option<&Awaiter> {
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

    /// Removes one awaiter and returns its waker.
    ///
    /// The awaiter transitions to the Notified state. The future
    /// that owns it will observe this on its next poll via
    /// [`Awaiter::take_notification()`] and complete with `Ready`.
    ///
    /// The caller should invoke [`Waker::wake()`] outside any lock
    /// scope to prevent re-entrancy deadlocks.
    ///
    /// Returns `None` if the set is empty.
    ///
    /// # Safety
    ///
    /// The set and all its awaiters must be protected by the same
    /// lock (or confined to a single thread).
    pub unsafe fn notify_one(&mut self) -> Option<Waker> {
        let awaiter = self.take_one()?;
        awaiter.notify()
    }

    /// Registers an awaiter with the given waker.
    ///
    /// If the awaiter is idle (first poll), it is inserted into the
    /// set. If it is already registered (subsequent poll), only the
    /// stored waker is updated.
    ///
    /// # Safety
    ///
    /// The awaiter and the set must be protected by the same lock
    /// (or confined to a single thread). The awaiter must remain
    /// pinned and valid until it is removed from the set.
    pub unsafe fn register(&mut self, awaiter: Pin<&mut Awaiter>, waker: Waker) {
        // SAFETY: Caller guarantees the same requirements.
        unsafe {
            self.register_with_data(awaiter, waker, 0);
        }
    }

    /// Registers an awaiter with a waker and caller-defined data.
    ///
    /// Behaves like [`register()`][Self::register] but also sets the
    /// awaiter's [`user_data`][Awaiter::user_data] (e.g. the number
    /// of permits a semaphore awaiter requests).
    ///
    /// # Safety
    ///
    /// The awaiter and the set must be protected by the same lock
    /// (or confined to a single thread). The awaiter must remain
    /// pinned and valid until it is removed from the set.
    pub unsafe fn register_with_data(
        &mut self,
        awaiter: Pin<&mut Awaiter>,
        waker: Waker,
        data: usize,
    ) {
        // SAFETY: We do not move the awaiter. Pin guarantees
        // address stability.
        let aw = unsafe { awaiter.get_unchecked_mut() };
        // SAFETY: Access is serialized by the caller's lock.
        let state = unsafe { &mut *aw.state.get() };
        match state {
            State::Idle => {
                *state = State::Waiting {
                    waker: Some(waker),
                    user_data: data,
                    next: ptr::null_mut(),
                    prev: ptr::null_mut(),
                };
                // SAFETY: Pin guarantees stable address.
                unsafe {
                    self.insert(ptr::from_mut(aw));
                }
            }
            State::Waiting {
                waker: w,
                user_data: ud,
                ..
            } => {
                *w = Some(waker);
                *ud = data;
            }
            State::Notified { .. } => {
                debug_assert!(false, "register called on notified awaiter");
            }
        }
    }

    /// Removes an awaiter from the set, returning it to the Idle
    /// state. No-op if the awaiter is not currently registered.
    ///
    /// Called by a future's drop handler when the future is cancelled
    /// before being notified.
    ///
    /// # Safety
    ///
    /// The awaiter and the set must be protected by the same lock
    /// (or confined to a single thread).
    pub unsafe fn unregister(&mut self, awaiter: Pin<&mut Awaiter>) {
        // SAFETY: We do not move the awaiter.
        let aw = unsafe { awaiter.get_unchecked_mut() };
        // SAFETY: Access is serialized by the caller's lock.
        if matches!(unsafe { &*aw.state.get() }, State::Waiting { .. }) {
            // SAFETY: The awaiter is in this set per the caller's
            // contract.
            unsafe {
                self.remove(ptr::from_mut(aw));
            }
            // SAFETY: Access is serialized by the caller's lock.
            unsafe {
                *aw.state.get() = State::Idle;
            }
        }
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
        assert!(unsafe { list.is_empty() });
        assert!(unsafe { list.peek() }.is_none());
    }

    #[test]
    fn default_list_is_empty() {
        let list = AwaiterSet::default();
        assert!(unsafe { list.is_empty() });
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
            list.register_with_data(Pin::new_unchecked(&mut a), waker(), 1);
        }
        assert!(!unsafe { list.is_empty() });
        assert_eq!(unsafe { list.peek().unwrap().user_data() }, 1);

        let popped = list.take_one();
        assert_eq!(unsafe { popped.unwrap().user_data() }, 1);
        assert!(unsafe { list.is_empty() });
    }

    #[test]
    fn fifo_ordering() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();
        let mut c = Awaiter::new();

        unsafe {
            list.register_with_data(Pin::new_unchecked(&mut a), waker(), 1);
            list.register_with_data(Pin::new_unchecked(&mut b), waker(), 2);
            list.register_with_data(Pin::new_unchecked(&mut c), waker(), 3);
        }

        assert!(!unsafe { list.is_empty() });

        assert_eq!(unsafe { list.take_one().unwrap().user_data() }, 1);
        assert_eq!(unsafe { list.take_one().unwrap().user_data() }, 2);
        assert_eq!(unsafe { list.take_one().unwrap().user_data() }, 3);

        assert!(unsafe { list.is_empty() });
        assert!(list.take_one().is_none());
    }

    #[test]
    fn remove_head_node() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();

        unsafe {
            list.register_with_data(Pin::new_unchecked(&mut a), waker(), 1);
            list.register_with_data(Pin::new_unchecked(&mut b), waker(), 2);
            list.unregister(Pin::new_unchecked(&mut a));
        }

        assert_eq!(unsafe { list.take_one().unwrap().user_data() }, 2);
        assert!(unsafe { list.is_empty() });
    }

    #[test]
    fn remove_tail_node() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();

        unsafe {
            list.register_with_data(Pin::new_unchecked(&mut a), waker(), 1);
            list.register_with_data(Pin::new_unchecked(&mut b), waker(), 2);
            list.unregister(Pin::new_unchecked(&mut b));
        }

        assert_eq!(unsafe { list.take_one().unwrap().user_data() }, 1);
        assert!(unsafe { list.is_empty() });
    }

    #[test]
    fn remove_middle_node() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();
        let mut c = Awaiter::new();

        unsafe {
            list.register_with_data(Pin::new_unchecked(&mut a), waker(), 1);
            list.register_with_data(Pin::new_unchecked(&mut b), waker(), 2);
            list.register_with_data(Pin::new_unchecked(&mut c), waker(), 3);
            list.unregister(Pin::new_unchecked(&mut b));
        }

        assert_eq!(unsafe { list.take_one().unwrap().user_data() }, 1);
        assert_eq!(unsafe { list.take_one().unwrap().user_data() }, 3);
        assert!(unsafe { list.is_empty() });
    }

    #[test]
    fn remove_only_node() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            list.register(Pin::new_unchecked(&mut a), waker());
            list.unregister(Pin::new_unchecked(&mut a));
        }

        assert!(unsafe { list.is_empty() });
    }

    #[test]
    fn remove_clears_node_links() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();

        unsafe {
            list.register(Pin::new_unchecked(&mut a), waker());
            list.register(Pin::new_unchecked(&mut b), waker());
            list.unregister(Pin::new_unchecked(&mut a));
        }

        assert!(a.next().is_null());
    }

    #[test]
    fn reuse_after_removal() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();

        unsafe {
            list.register(Pin::new_unchecked(&mut a), waker());
            list.unregister(Pin::new_unchecked(&mut a));
            list.register_with_data(Pin::new_unchecked(&mut b), waker(), 2);
        }

        assert_eq!(unsafe { list.take_one().unwrap().user_data() }, 2);
        assert!(unsafe { list.is_empty() });
    }

    #[test]
    fn interleaved_push_and_pop() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();
        let mut c = Awaiter::new();

        unsafe {
            list.register_with_data(Pin::new_unchecked(&mut a), waker(), 1);
            list.register_with_data(Pin::new_unchecked(&mut b), waker(), 2);
        }

        assert_eq!(unsafe { list.take_one().unwrap().user_data() }, 1);

        unsafe {
            list.register_with_data(Pin::new_unchecked(&mut c), waker(), 3);
        }

        assert_eq!(unsafe { list.take_one().unwrap().user_data() }, 2);
        assert_eq!(unsafe { list.take_one().unwrap().user_data() }, 3);
        assert!(unsafe { list.is_empty() });
    }

    #[test]
    fn traversal_via_next() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();
        let mut c = Awaiter::new();

        unsafe {
            list.register_with_data(Pin::new_unchecked(&mut a), waker(), 1);
            list.register_with_data(Pin::new_unchecked(&mut b), waker(), 2);
            list.register_with_data(Pin::new_unchecked(&mut c), waker(), 3);
        }

        let head = unsafe { list.peek() }.unwrap();
        assert_eq!(unsafe { head.user_data() }, 1);

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
            list.register(Pin::new_unchecked(&mut a), waker());
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
            list.register_with_data(Pin::new_unchecked(&mut a), waker(), 7);
        }

        let popped = list.take_one().unwrap();
        assert_eq!(unsafe { popped.user_data() }, 7);
    }

    #[test]
    fn notified_flag_survives_list_operations() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            list.register(Pin::new_unchecked(&mut a), waker());
        }

        drop(unsafe { list.notify_one() });
        // SAFETY: The awaiter is not moved.
        assert!(unsafe { Pin::new_unchecked(&a).is_notified() });
    }

    #[test]
    fn ten_elements_maintain_fifo() {
        let mut list = AwaiterSet::new();
        let mut nodes: Vec<Awaiter> = std::iter::repeat_with(Awaiter::new).take(10).collect();

        for (i, node) in nodes.iter_mut().enumerate() {
            unsafe {
                list.register_with_data(Pin::new_unchecked(node), waker(), i);
            }
        }

        for i in 0..10 {
            assert_eq!(unsafe { list.take_one().unwrap().user_data() }, i);
        }

        assert!(unsafe { list.is_empty() });
    }

    #[test]
    fn debug_output_does_not_panic() {
        let list = AwaiterSet::new();
        let debug = format!("{list:?}");
        assert!(debug.contains("AwaiterSet"));
    }
}
