use std::any::type_name;
use std::fmt;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::ptr;
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
///     // SAFETY: We hold the lock that protects the set.
///     if unsafe { awaiter.as_mut().take_notification() } {
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
    // the tail and notify_one() removes from the head, so the current
    // implementation notifies in FIFO order. This is not an API
    // guarantee but tests do verify FIFO behavior — update them if
    // the ordering ever changes.
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

        let head = self.head;

        // SAFETY: `head` is non-null, so it is a valid awaiter.
        let head_ref = unsafe { &*head };
        // SAFETY: Access is serialized by the caller's lock.
        let state = unsafe { head_ref.state_mut() };
        let next = state.next;

        self.head = next;

        if next.is_null() {
            self.tail = ptr::null_mut();
        } else {
            // SAFETY: `next` is a valid awaiter in the set.
            let next = unsafe { &*next };
            // SAFETY: Access is serialized by the caller's lock.
            unsafe { next.state_mut() }.prev = ptr::null_mut();
        }

        // Transition the removed awaiter to notified.
        state.next = ptr::null_mut();
        state.prev = ptr::null_mut();
        state.notified = true;
        state.waker.take()
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

        // SAFETY: Access is serialized by the caller's lock.
        let state = unsafe { awaiter.state_mut() };

        if state.registered {
            // Already registered — update waker and data in place.
            state.waker = Some(waker);
            state.user_data = data;
        } else {
            // New registration — initialize fields and insert.
            *state = State {
                waker: Some(waker),
                user_data: data,
                next: ptr::null_mut(),
                prev: ptr::null_mut(),
                registered: true,
                notified: false,
            };

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
    /// state. No-op if the awaiter is not currently registered
    /// in the waiting state.
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
        let awaiter = unsafe { awaiter.get_unchecked_mut() };
        // SAFETY: Access is serialized by the caller's lock.
        let state = unsafe { awaiter.state_ref() };

        // Only remove awaiters that are waiting (registered but not
        // yet notified). Idle and notified awaiters are not in the
        // set.
        if !state.registered || state.notified {
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
        assert!(unsafe { list.peek() }.is_none());
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
        assert_eq!(unsafe { list.peek().unwrap().user_data() }, 1);

        let waker = unsafe { list.notify_one() };
        assert!(waker.is_some());
        assert_eq!(unsafe { a.user_data() }, 1);
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

        drop(unsafe { list.notify_one() });
        assert_eq!(unsafe { a.user_data() }, 1);
        drop(unsafe { list.notify_one() });
        assert_eq!(unsafe { b.user_data() }, 2);
        drop(unsafe { list.notify_one() });
        assert_eq!(unsafe { c.user_data() }, 3);

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

        drop(unsafe { list.notify_one() });
        assert_eq!(unsafe { b.user_data() }, 2);
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

        drop(unsafe { list.notify_one() });
        assert_eq!(unsafe { a.user_data() }, 1);
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

        drop(unsafe { list.notify_one() });
        assert_eq!(unsafe { a.user_data() }, 1);
        drop(unsafe { list.notify_one() });
        assert_eq!(unsafe { c.user_data() }, 3);
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
        assert!(!unsafe { a.is_registered() });
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
    fn interleaved_push_and_pop() {
        let mut list = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();
        let mut c = Awaiter::new();

        unsafe {
            list.register_with_data(Pin::new_unchecked(&mut a), waker(), 1);
            list.register_with_data(Pin::new_unchecked(&mut b), waker(), 2);
        }

        drop(unsafe { list.notify_one() });
        assert_eq!(unsafe { a.user_data() }, 1);

        unsafe {
            list.register_with_data(Pin::new_unchecked(&mut c), waker(), 3);
        }

        drop(unsafe { list.notify_one() });
        assert_eq!(unsafe { b.user_data() }, 2);
        drop(unsafe { list.notify_one() });
        assert_eq!(unsafe { c.user_data() }, 3);
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

        let second = unsafe { head.state_ref() }.next;
        assert!(!second.is_null());
        assert_eq!(unsafe { (*second).user_data() }, 2);

        let third = unsafe { (*second).state_ref() }.next;
        assert!(!third.is_null());
        assert_eq!(unsafe { (*third).user_data() }, 3);

        let end = unsafe { (*third).state_ref() }.next;
        assert!(end.is_null());
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
        assert!(unsafe { Pin::new_unchecked(&a).is_notified() });
    }

    #[test]
    fn ten_elements_maintain_fifo() {
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

        for (i, node) in nodes.iter().enumerate() {
            drop(unsafe { list.notify_one() });
            assert_eq!(unsafe { node.user_data() }, i);
        }

        assert!(unsafe { list.is_empty() });
    }

    #[test]
    fn debug_output_does_not_panic() {
        let list = AwaiterSet::new();
        let debug = format!("{list:?}");
        assert!(debug.contains("AwaiterSet"));
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
        assert!(unsafe { Pin::new_unchecked(&a).is_notified() });
    }

    #[test]
    fn peek_updates_after_notify_one() {
        let mut set = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();
        let mut c = Awaiter::new();

        unsafe {
            set.register_with_data(Pin::new_unchecked(&mut a), waker(), 1);
            set.register_with_data(Pin::new_unchecked(&mut b), waker(), 2);
            set.register_with_data(Pin::new_unchecked(&mut c), waker(), 3);
        }

        assert_eq!(unsafe { set.peek().unwrap().user_data() }, 1);

        // Remove first — peek should now return second.
        drop(unsafe { set.notify_one() });
        assert_eq!(unsafe { set.peek().unwrap().user_data() }, 2);

        // Remove second — peek should now return third.
        drop(unsafe { set.notify_one() });
        assert_eq!(unsafe { set.peek().unwrap().user_data() }, 3);

        // Remove third — set is empty.
        drop(unsafe { set.notify_one() });
        assert!(unsafe { set.peek() }.is_none());
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

                if unsafe { awaiter.as_mut().take_notification() } {
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
