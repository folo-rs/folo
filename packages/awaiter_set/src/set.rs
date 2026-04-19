use std::any::type_name;
use std::fmt;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::ptr;
use std::sync::atomic::Ordering;
use std::task::Waker;

use crate::awaiter::{Awaiter, State};

/// Tracks awaiters for a synchronization primitive.
///
/// Owned by an asynchronous synchronization primitive (e.g. a mutex or event).
/// Asynchronous futures that cannot complete immediately
/// [`register()`][Self::register] an [`Awaiter`] with this set.
/// When the primitive grants a resource, it calls
/// [`notify_one()`][Self::notify_one] to remove an awaiter, set
/// it into a notified state and return its [`Waker`] so the primitive
/// can wake the corresponding future.
///
/// # Synchronization
///
/// The set has no internal synchronization. The primitive must
/// serialize all access to both the set and every [`Awaiter`]
/// registered with it — for example, by protecting them with a
/// single synchronous [`Mutex`][std::sync::Mutex] or by confining the
/// synchronization primitive and all its futures to a single thread.
///
/// # Examples
///
/// An awaiter set shared between an async future and a notifying
/// thread, synchronized via a [`Mutex`][std::sync::Mutex].
///
/// ```
/// use std::future::poll_fn;
/// use std::pin::Pin;
/// use std::sync::{Arc, Mutex};
/// use std::task::Poll;
/// use std::thread;
///
/// use awaiter_set::{Awaiter, AwaiterSet};
/// # use futures::executor::block_on;
///
/// let set = Arc::new(Mutex::new(AwaiterSet::new()));
///
/// // Spawn a thread that will notify one awaiter after a
/// // brief moment. It acquires the lock, calls notify_one,
/// // then releases the lock before waking to avoid
/// // reentrancy deadlocks.
/// thread::spawn({
///     let set = Arc::clone(&set);
///     move || {
///         let waker = {
///             let mut guard = set.lock().unwrap();
///             // SAFETY: We hold the lock that protects the set.
///             loop {
///                 match unsafe { guard.notify_one() } {
///                     Some(w) => break w,
///                     None => {
///                         drop(guard);
///                         thread::yield_now();
///                         guard = set.lock().unwrap();
///                     }
///                 }
///             }
///         };
///         // Wake outside the lock to prevent deadlocks.
///         waker.wake();
///     }
/// });
///
/// // On the main thread, run an async task that registers an
/// // awaiter and waits to be notified.
/// # block_on(async {
/// let mut awaiter = Box::pin(Awaiter::new());
///
/// poll_fn(|cx| {
///     let waker = cx.waker().clone();
///     let mut guard = set.lock().unwrap();
///
///     if awaiter.as_ref().take_notification() {
///         return Poll::Ready(());
///     }
///
///     // SAFETY: We hold the lock. The awaiter remains pinned
///     // and valid until removed from the set.
///     unsafe {
///         guard.register(awaiter.as_mut(), waker);
///     }
///
///     Poll::Pending
/// })
/// .await;
/// # });
/// ```
pub struct AwaiterSet {
    // Both are null if the set is empty. New entries are appended at
    // the tail. In release builds, notify_one() removes from the head
    // (FIFO). In debug builds, it alternates between head and tail
    // based on pointer address to catch code that depends on ordering.
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
    ///
    /// // SAFETY: Single-threaded example requires no synchronization.
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

    /// Removes one awaiter matching a condition and returns its waker.
    ///
    /// Walks the set and calls `predicate` on each awaiter. The first
    /// awaiter for which the predicate returns `true` is removed,
    /// transitioned to the notified state, and its waker is returned.
    /// This allows callers to skip awaiters that cannot be satisfied
    /// (e.g. a semaphore awaiter requesting more permits than are
    /// available).
    ///
    /// The caller should invoke [`Waker::wake()`] outside any lock
    /// scope to prevent reentrancy deadlocks.
    ///
    /// Returns `None` if the set is empty or no awaiter matches.
    ///
    /// # Safety
    ///
    /// The set and all its awaiters must be protected by the same
    /// lock (or confined to a single thread). The predicate must
    /// not modify any shared state or call methods on the set.
    ///
    /// # Examples
    ///
    /// Notify the first awaiter whose requested permit count
    /// (stored as user data) fits within the available budget.
    ///
    /// ```
    /// use std::pin::Pin;
    /// use std::task::Waker;
    ///
    /// use awaiter_set::{Awaiter, AwaiterSet};
    ///
    /// let mut set = AwaiterSet::new();
    /// let mut small = Awaiter::new();
    /// let mut big = Awaiter::new();
    ///
    /// // SAFETY: Single-threaded example.
    /// unsafe {
    ///     // Register two awaiters with different permit counts.
    ///     set.register_with_data(
    ///         Pin::new_unchecked(&mut big),
    ///         Waker::noop().clone(),
    ///         5,
    ///     );
    ///     set.register_with_data(
    ///         Pin::new_unchecked(&mut small),
    ///         Waker::noop().clone(),
    ///         2,
    ///     );
    /// }
    ///
    /// let available = 3;
    ///
    /// // Only wake an awaiter that needs at most 3 permits.
    /// // SAFETY: Single-threaded example.
    /// let waker = unsafe {
    ///     set.notify_one_if(|awaiter| {
    ///         awaiter.user_data() <= available
    ///     })
    /// };
    ///
    /// assert!(waker.is_some());
    /// // The small awaiter (requesting 2) was notified.
    /// // SAFETY: Single-threaded example.
    /// assert!(unsafe { Pin::new_unchecked(&small).is_notified() });
    /// // The big awaiter (requesting 5) is still waiting.
    /// assert!(!unsafe { Pin::new_unchecked(&big).is_notified() });
    /// ```
    pub unsafe fn notify_one_if(
        &mut self,
        mut predicate: impl FnMut(&Awaiter) -> bool,
    ) -> Option<Waker> {
        let mut current = self.head;
        while !current.is_null() {
            // SAFETY: All pointers in the set are valid (set invariant).
            let awaiter = unsafe { &*current };
            if predicate(awaiter) {
                return self.remove_and_notify(current);
            }
            // SAFETY: Access is serialized by the caller's lock.
            current = unsafe { awaiter.state_ref() }.next;
        }
        None
    }

    /// Inserts an awaiter into the set.
    ///
    /// # Safety
    ///
    /// The awaiter must remain valid and pinned until removed from
    /// the set.
    unsafe fn insert(&mut self, awaiter: Pin<&mut Awaiter>) {
        // SAFETY: We do not move the awaiter. We only store the
        // pointer for the linked structure.
        let awaiter = unsafe { ptr::from_mut(awaiter.get_unchecked_mut()) };

        // SAFETY: awaiter pointer is valid (derived from Pin).
        let awaiter_ref = unsafe { &*awaiter };
        // SAFETY: Access is serialized by the caller's lock.
        let state = unsafe { awaiter_ref.state_mut() };
        state.next = ptr::null_mut();
        state.prev = self.tail;

        if self.tail.is_null() {
            self.head = awaiter;
        } else {
            // SAFETY: `tail` is non-null and valid (set invariant).
            let tail = unsafe { &*self.tail };
            // SAFETY: Access is serialized by the caller's lock.
            unsafe { tail.state_mut() }.next = awaiter;
        }

        self.tail = awaiter;
    }

    /// Removes a specific awaiter from the set.
    ///
    /// # Safety
    ///
    /// The awaiter must currently be in this set.
    unsafe fn remove(&mut self, awaiter: Pin<&mut Awaiter>) {
        // SAFETY: We do not move the awaiter.
        let awaiter = unsafe { ptr::from_mut(awaiter.get_unchecked_mut()) };
        // SAFETY: awaiter pointer is valid.
        let awaiter_ref = unsafe { &*awaiter };
        // SAFETY: Access is serialized by the caller's lock.
        let state = unsafe { awaiter_ref.state_ref() };
        let next = state.next;
        let prev = state.prev;

        if prev.is_null() {
            self.head = next;
        } else {
            // SAFETY: `prev` is a valid awaiter in the set.
            let prev = unsafe { &*prev };
            // SAFETY: Access is serialized by the caller's lock.
            unsafe { prev.state_mut() }.next = next;
        }

        if next.is_null() {
            self.tail = prev;
        } else {
            // SAFETY: `next` is a valid awaiter in the set.
            let next = unsafe { &*next };
            // SAFETY: Access is serialized by the caller's lock.
            unsafe { next.state_mut() }.prev = prev;
        }

        // Clear the removed node's links.
        // SAFETY: Access is serialized by the caller's lock.
        let state = unsafe { awaiter_ref.state_mut() };
        state.next = ptr::null_mut();
        state.prev = ptr::null_mut();
    }

    /// Removes one awaiter and returns its waker.
    ///
    /// The awaiter transitions to the notified state. The future
    /// that owns it will observe this on its next poll via
    /// [`Awaiter::take_notification()`] and complete with `Ready`.
    ///
    /// The caller should invoke [`Waker::wake()`] outside any lock
    /// scope to prevent reentrancy deadlocks.
    ///
    /// Returns `None` if the set is empty.
    ///
    /// # Safety
    ///
    /// The set and all its awaiters must be protected by the same
    /// lock (or confined to a single thread).
    pub unsafe fn notify_one(&mut self) -> Option<Waker> {
        if self.head.is_null() {
            return None;
        }

        self.remove_and_notify(self.pick_one())
    }

    // Removes a specific awaiter from the set by pointer and
    // transitions it to the notified state. Returns the stored waker.
    //
    // Mutating to return None causes all notification paths to hang
    // because waiters are never woken.
    #[cfg_attr(test, mutants::skip)]
    fn remove_and_notify(&mut self, ptr: *mut Awaiter) -> Option<Waker> {
        // SAFETY: ptr is a valid awaiter in the set (caller invariant).
        let awaiter = unsafe { &*ptr };
        // SAFETY: Access is serialized by the caller's lock.
        let state = unsafe { awaiter.state_ref() };
        let next = state.next;
        let prev = state.prev;

        // Unlink the awaiter from the doubly-linked list.
        if prev.is_null() {
            self.head = next;
        } else {
            // SAFETY: `prev` is a valid awaiter in the set.
            let prev_ref = unsafe { &*prev };
            // SAFETY: Access is serialized by the caller's lock.
            unsafe { prev_ref.state_mut() }.next = next;
        }

        if next.is_null() {
            self.tail = prev;
        } else {
            // SAFETY: `next` is a valid awaiter in the set.
            let next_ref = unsafe { &*next };
            // SAFETY: Access is serialized by the caller's lock.
            unsafe { next_ref.state_mut() }.prev = prev;
        }

        // Clean up the State data fields first, before publishing
        // the notification via the atomic lifecycle store.
        // SAFETY: Access is serialized by the caller's lock.
        let state = unsafe { awaiter.state_mut() };
        let waker = state.waker.take();
        state.next = ptr::null_mut();
        state.prev = ptr::null_mut();

        // Publish the notification. The Release ordering ensures all
        // State modifications above are visible to the polled task
        // that loads with Acquire via take_notification().
        awaiter.set_lifecycle(crate::awaiter::LIFECYCLE_NOTIFIED, Ordering::Release);

        waker
    }

    // In debug builds, pick head or tail based on pointer address to
    // flush out tests that depend on notification order. In release
    // builds, always pick head (FIFO).
    //
    // The exact comparison logic is a testing aid and has no
    // correctness implications — any choice is valid.
    #[cfg_attr(test, mutants::skip)]
    fn pick_one(&self) -> *mut Awaiter {
        #[cfg(debug_assertions)]
        {
            if self.head == self.tail {
                return self.head;
            }
            if (self.head as usize) > (self.tail as usize) {
                self.head
            } else {
                self.tail
            }
        }
        #[cfg(not(debug_assertions))]
        {
            self.head
        }
    }

    /// Registers an awaiter with the given waker.
    ///
    /// If the awaiter is idle, it is inserted into the set and the
    /// waker is stored. If the awaiter is already registered, only
    /// the stored waker is replaced.
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
        let awaiter = unsafe { awaiter.get_unchecked_mut() };

        let lifecycle = awaiter.lifecycle_phase();

        if lifecycle == crate::awaiter::LIFECYCLE_WAITING {
            // Already registered — update waker and data in place.
            // SAFETY: Access is serialized by the caller's lock.
            let state = unsafe { awaiter.state_mut() };
            state.waker = Some(waker);
            state.user_data = data;
        } else {
            // New registration — initialize fields and insert.
            // SAFETY: Access is serialized by the caller's lock.
            let state = unsafe { awaiter.state_mut() };
            *state = State {
                waker: Some(waker),
                user_data: data,
                next: ptr::null_mut(),
                prev: ptr::null_mut(),
            };
            awaiter.set_lifecycle(crate::awaiter::LIFECYCLE_WAITING, Ordering::Relaxed);

            // SAFETY: The awaiter was originally pinned and has not
            // been moved.
            let awaiter = unsafe { Pin::new_unchecked(awaiter) };
            // SAFETY: The awaiter will remain valid until removed.
            unsafe {
                self.insert(awaiter);
            }
        }
    }

    /// Removes an awaiter from the set, returning it to the idle
    /// state.
    ///
    /// If the awaiter has been notified (removed from the set by
    /// [`notify_one()`][Self::notify_one]), this is a no-op because
    /// the awaiter is no longer in the set.
    ///
    /// # Panics
    ///
    /// Panics (in debug builds) if the awaiter is idle — calling
    /// unregister on an awaiter that was never registered is a logic
    /// error.
    ///
    /// # Safety
    ///
    /// The awaiter and the set must be protected by the same lock
    /// (or confined to a single thread).
    pub unsafe fn unregister(&mut self, awaiter: Pin<&mut Awaiter>) {
        // SAFETY: We do not move the awaiter.
        let awaiter = unsafe { awaiter.get_unchecked_mut() };

        let lifecycle = awaiter.lifecycle_phase();

        debug_assert!(
            lifecycle != crate::awaiter::LIFECYCLE_IDLE,
            "unregister called on an idle awaiter"
        );

        // Notified awaiters have already been removed from the set
        // by notify_one(). Nothing to do.
        if lifecycle == crate::awaiter::LIFECYCLE_NOTIFIED {
            return;
        }

        // The awaiter is still registered — guard against idle
        // callers in release builds.
        if lifecycle != crate::awaiter::LIFECYCLE_WAITING {
            return;
        }

        let awaiter = ptr::from_mut(awaiter);

        // SAFETY: awaiter pointer is valid.
        let awaiter_mut = unsafe { &mut *awaiter };
        // SAFETY: The awaiter was originally pinned.
        let awaiter_pin = unsafe { Pin::new_unchecked(awaiter_mut) };
        // SAFETY: The awaiter is in this set.
        unsafe {
            self.remove(awaiter_pin);
        }

        // SAFETY: The awaiter is still valid at the same address.
        // remove() cleared its pointers but did not move it.
        let awaiter = unsafe { &*awaiter };
        // SAFETY: Access is serialized by the caller's lock.
        let state = unsafe { awaiter.state_mut() };
        *state = State::idle();
        awaiter.set_lifecycle(crate::awaiter::LIFECYCLE_IDLE, Ordering::Relaxed);
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

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for AwaiterSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
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
    use std::iter;
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
    }

    #[test]
    fn default_list_is_empty() {
        let list = AwaiterSet::default();
        assert!(unsafe { list.is_empty() });
    }

    #[test]
    fn notify_one_on_empty_returns_none() {
        let mut list = AwaiterSet::new();
        assert!(unsafe { list.notify_one() }.is_none());
    }

    #[test]
    fn register_and_notify_single_element() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            list.register_with_data(Pin::new_unchecked(&mut a), waker(), 1);
        }
        assert!(!unsafe { list.is_empty() });

        let waker = unsafe { list.notify_one() };
        assert!(waker.is_some());
        assert_eq!(unsafe { a.user_data() }, 1);
        assert!(unsafe { list.is_empty() });
    }

    #[test]
    fn all_registered_awaiters_are_notified() {
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

        let mut notified = Vec::new();
        while let Some(w) = unsafe { list.notify_one() } {
            drop(w);
        }
        for aw in [&a, &b, &c] {
            notified.push(unsafe { aw.user_data() });
        }

        notified.sort_unstable();
        assert_eq!(notified, [1, 2, 3]);

        assert!(unsafe { list.is_empty() });
        assert!(unsafe { list.notify_one() }.is_none());
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

        assert!(!a.is_registered());
        let w = unsafe { list.notify_one() };
        assert!(w.is_some());
        assert!(b.is_registered());
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

        assert!(!b.is_registered());
        let w = unsafe { list.notify_one() };
        assert!(w.is_some());
        assert!(a.is_registered());
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

        assert!(!b.is_registered());
        // Both remaining awaiters should be notified.
        drop(unsafe { list.notify_one() });
        drop(unsafe { list.notify_one() });
        assert!(a.is_registered());
        assert!(c.is_registered());
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
    fn unregister_returns_to_idle() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();

        unsafe {
            list.register(Pin::new_unchecked(&mut a), waker());
            list.register(Pin::new_unchecked(&mut b), waker());
            list.unregister(Pin::new_unchecked(&mut a));
        }

        // After unregister, the awaiter is back in Idle state.
        assert!(!a.is_registered());
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

        drop(unsafe { list.notify_one() });
        assert_eq!(unsafe { b.user_data() }, 2);
        assert!(unsafe { list.is_empty() });
    }

    #[test]
    fn interleaved_register_and_notify() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();
        let mut c = Awaiter::new();

        unsafe {
            list.register_with_data(Pin::new_unchecked(&mut a), waker(), 1);
            list.register_with_data(Pin::new_unchecked(&mut b), waker(), 2);
        }

        // Notify one of the two registered awaiters.
        drop(unsafe { list.notify_one() });

        // Register a third while one is still in the set.
        unsafe {
            list.register_with_data(Pin::new_unchecked(&mut c), waker(), 3);
        }

        // Drain remaining.
        drop(unsafe { list.notify_one() });
        drop(unsafe { list.notify_one() });
        assert!(unsafe { list.is_empty() });

        // All three must have been notified (order is unspecified).
        let mut notified: Vec<_> = [&a, &b, &c]
            .iter()
            .map(|aw| unsafe { aw.user_data() })
            .collect();
        notified.sort_unstable();
        assert_eq!(notified, [1, 2, 3]);
    }

    #[test]
    fn all_elements_reachable_via_traversal() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();
        let mut c = Awaiter::new();

        unsafe {
            list.register_with_data(Pin::new_unchecked(&mut a), waker(), 1);
            list.register_with_data(Pin::new_unchecked(&mut b), waker(), 2);
            list.register_with_data(Pin::new_unchecked(&mut c), waker(), 3);
        }

        // Traverse from head via next pointers and collect user_data.
        let mut values = Vec::new();
        let mut current = list.head;
        while !current.is_null() {
            values.push(unsafe { (*current).user_data() });
            current = unsafe { (*current).state_ref() }.next;
        }

        values.sort_unstable();
        assert_eq!(values, [1, 2, 3]);
    }

    #[test]
    fn wakers_survive_notify() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            list.register(Pin::new_unchecked(&mut a), waker());
        }

        let recovered = unsafe { list.notify_one() };
        assert!(recovered.is_some());
    }

    #[test]
    fn user_data_survives_notify() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            list.register_with_data(Pin::new_unchecked(&mut a), waker(), 7);
        }

        drop(unsafe { list.notify_one() });
        assert_eq!(unsafe { a.user_data() }, 7);
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
        assert!(unsafe { Pin::new_unchecked(&a) }.is_notified());
    }

    #[test]
    fn ten_elements_all_notified() {
        const ELEMENT_COUNT: usize = 10;
        let mut list = AwaiterSet::new();
        let mut nodes: Vec<Awaiter> = iter::repeat_with(Awaiter::new)
            .take(ELEMENT_COUNT)
            .collect();

        for (i, node) in nodes.iter_mut().enumerate() {
            unsafe {
                list.register_with_data(Pin::new_unchecked(node), waker(), i);
            }
        }

        for _ in 0..ELEMENT_COUNT {
            let w = unsafe { list.notify_one() };
            assert!(w.is_some());
        }

        assert!(unsafe { list.is_empty() });

        // All awaiters received their original user_data.
        let mut data: Vec<_> = nodes.iter().map(|n| unsafe { n.user_data() }).collect();
        data.sort_unstable();
        assert_eq!(data, (0..ELEMENT_COUNT).collect::<Vec<_>>());
    }

    #[test]
    fn debug_output_does_not_panic() {
        let list = AwaiterSet::new();
        let debug = format!("{list:?}");
        assert!(debug.contains("AwaiterSet"));
    }

    #[test]
    fn notify_one_if_skips_non_matching() {
        let mut set = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();
        let mut c = Awaiter::new();

        unsafe {
            set.register_with_data(Pin::new_unchecked(&mut a), waker(), 10);
            set.register_with_data(Pin::new_unchecked(&mut b), waker(), 5);
            set.register_with_data(Pin::new_unchecked(&mut c), waker(), 3);
        }

        // Only notify awaiters with user_data <= 4.
        let w = unsafe { set.notify_one_if(|aw| aw.user_data() <= 4) };
        assert!(w.is_some());

        // The awaiter with user_data 3 should have been notified.
        assert!(unsafe { Pin::new_unchecked(&c).is_notified() });
        assert!(!unsafe { Pin::new_unchecked(&a) }.is_notified());
        assert!(!unsafe { Pin::new_unchecked(&b).is_notified() });
    }

    #[test]
    fn notify_one_if_returns_none_when_no_match() {
        let mut set = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            set.register_with_data(Pin::new_unchecked(&mut a), waker(), 10);
        }

        // No awaiter with user_data <= 4.
        let w = unsafe { set.notify_one_if(|aw| aw.user_data() <= 4) };
        assert!(w.is_none());

        // The awaiter should still be registered.
        assert!(a.is_registered());
        assert!(!unsafe { set.is_empty() });
    }

    #[test]
    fn notify_one_if_on_empty_returns_none() {
        let mut set = AwaiterSet::new();
        let w = unsafe { set.notify_one_if(|_| true) };
        assert!(w.is_none());
    }

    #[test]
    fn notify_one_single_element() {
        let mut set = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), waker());
        }
        assert!(!unsafe { set.is_empty() });

        let w = unsafe { set.notify_one() };
        assert!(w.is_some());
        assert!(unsafe { set.is_empty() });
        assert!(unsafe { Pin::new_unchecked(&a) }.is_notified());
    }

    #[test]
    fn re_register_updates_user_data() {
        let mut set = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            set.register_with_data(Pin::new_unchecked(&mut a), waker(), 42);
        }
        assert_eq!(unsafe { a.user_data() }, 42);

        // Re-register with different user_data.
        unsafe {
            set.register_with_data(Pin::new_unchecked(&mut a), waker(), 99);
        }
        assert_eq!(unsafe { a.user_data() }, 99);

        // Notify and verify updated data persists.
        drop(unsafe { set.notify_one() });
        assert_eq!(unsafe { a.user_data() }, 99);
    }

    #[test]
    fn multithreaded_register_notify() {
        use std::sync::{Arc, Barrier, Mutex};
        use std::thread;

        // Shared state: Mutex<AwaiterSet> + a flag.
        struct Shared {
            set: AwaiterSet,
            notified: bool,
        }

        let shared = Arc::new(Mutex::new(Shared {
            set: AwaiterSet::new(),
            notified: false,
        }));
        let barrier = Arc::new(Barrier::new(2));

        // Thread 1: register an awaiter, wait for notification.
        let handle = thread::spawn({
            let shared = Arc::clone(&shared);
            let barrier = Arc::clone(&barrier);
            move || {
                let mut awaiter = Awaiter::new();

                {
                    let mut guard = shared.lock().unwrap();
                    // SAFETY: We hold the lock.
                    unsafe {
                        guard
                            .set
                            .register(Pin::new_unchecked(&mut awaiter), Waker::noop().clone());
                    }
                }

                // Signal that we are registered.
                barrier.wait();
                // Wait for thread 2 to notify.
                barrier.wait();

                let guard = shared.lock().unwrap();
                assert!(guard.notified);
                // SAFETY: We hold the lock.
                assert!(unsafe { Pin::new_unchecked(&awaiter).is_notified() });
            }
        });

        // Thread 2: wait for registration, then notify.
        barrier.wait();
        {
            let mut guard = shared.lock().unwrap();
            // SAFETY: We hold the lock.
            let waker = unsafe { guard.set.notify_one() };
            assert!(waker.is_some());
            guard.notified = true;
        }
        barrier.wait();

        handle.join().unwrap();
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn async_register_notify_across_threads() {
        use std::future::poll_fn;
        use std::sync::{Arc, Mutex};
        use std::task::Poll;
        use std::thread;

        use futures::executor::block_on;

        let set = Arc::new(Mutex::new(AwaiterSet::new()));

        thread::spawn({
            let set = Arc::clone(&set);
            move || {
                let waker = loop {
                    let mut guard = set.lock().unwrap();
                    if let Some(w) = unsafe { guard.notify_one() } {
                        break w;
                    }
                    drop(guard);
                    thread::yield_now();
                };
                waker.wake();
            }
        });

        block_on(async {
            let mut awaiter = Box::pin(Awaiter::new());

            poll_fn(|cx| {
                let waker = cx.waker().clone();
                let mut guard = set.lock().unwrap();

                if awaiter.as_ref().take_notification() {
                    return Poll::Ready(());
                }

                unsafe {
                    guard.register(awaiter.as_mut(), waker);
                }

                Poll::Pending
            })
            .await;
        });
    }
}
