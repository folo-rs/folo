use std::any::type_name;
use std::fmt;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::ptr;
use std::sync::atomic::Ordering;
use std::task::Waker;

use crate::awaiter::{Awaiter, IDLE, Inner, NOTIFIED, WAITING};

/// Tracks awaiters for a synchronization primitive.
///
/// Owned by an asynchronous synchronization primitive (for example an
/// event). Asynchronous futures that cannot complete immediately
/// [`register()`][Self::register] an [`Awaiter`] with this set. When
/// the primitive grants its resource, it calls
/// [`notify_one()`][Self::notify_one] to remove an awaiter, transition
/// it to the notified state, and return its [`Waker`] so the primitive
/// can wake the corresponding future.
///
/// # Synchronization
///
/// The set has no internal synchronization. The owning primitive must
/// serialize all access to both the set and every [`Awaiter`]
/// registered with it — for example by protecting them with a single
/// synchronous [`Mutex`][std::sync::Mutex] or by confining the
/// primitive and its futures to a single thread.
///
/// # Examples
///
/// An awaiter set shared between an async future and a notifying
/// thread, synchronized via a [`Mutex`][std::sync::Mutex]:
///
/// ```
/// use std::future::poll_fn;
/// use std::sync::{Arc, Mutex};
/// use std::task::Poll;
/// use std::thread;
///
/// use awaiter_set::{Awaiter, AwaiterSet};
/// # use futures::executor::block_on;
///
/// let set = Arc::new(Mutex::new(AwaiterSet::new()));
///
/// // Notifier thread: acquire lock, take a waker, release lock,
/// // then wake outside the lock to avoid reentrancy deadlocks.
/// thread::spawn({
///     let set = Arc::clone(&set);
///     move || {
///         let waker = loop {
///             let mut guard = set.lock().unwrap();
///             // SAFETY: We hold the lock that protects the set.
///             if let Some(w) = unsafe { guard.notify_one() } {
///                 break w;
///             }
///             drop(guard);
///             thread::yield_now();
///         };
///         waker.wake();
///     }
/// });
///
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
///     // SAFETY: We hold the lock. The awaiter remains pinned and
///     // valid until removed from the set.
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
        // SAFETY: We do not move the awaiter. Pin guarantees address
        // stability.
        let awaiter = unsafe { awaiter.get_unchecked_mut() };

        if awaiter.lifecycle_phase() == WAITING {
            // Already registered — update the waker in place.
            // SAFETY: Access is serialized by the caller's lock.
            unsafe { awaiter.inner_mut() }.waker = Some(waker);
            return;
        }

        // New registration — initialize fields and insert.
        // SAFETY: Access is serialized by the caller's lock.
        let inner = unsafe { awaiter.inner_mut() };
        inner.waker = Some(waker);
        inner.next = ptr::null_mut();
        inner.prev = self.tail;

        let ptr = ptr::from_mut(awaiter);

        if self.tail.is_null() {
            self.head = ptr;
        } else {
            // SAFETY: `tail` is non-null and valid (set invariant).
            let tail = unsafe { &*self.tail };
            // SAFETY: Access is serialized by the caller's lock.
            unsafe { tail.inner_mut() }.next = ptr;
        }
        self.tail = ptr;

        awaiter.set_lifecycle(WAITING, Ordering::Relaxed);
    }

    /// Removes an awaiter from the set, returning it to the idle
    /// state.
    ///
    /// If the awaiter has already been notified (and therefore
    /// already removed from the set by
    /// [`notify_one()`][Self::notify_one]), this is a no-op.
    ///
    /// # Panics
    ///
    /// Panics (in debug builds) if the awaiter is idle — calling
    /// `unregister` on an awaiter that was never registered is a
    /// logic error.
    ///
    /// # Safety
    ///
    /// The awaiter and the set must be protected by the same lock
    /// (or confined to a single thread).
    pub unsafe fn unregister(&mut self, awaiter: Pin<&mut Awaiter>) {
        // SAFETY: We do not move the awaiter.
        let awaiter = unsafe { awaiter.get_unchecked_mut() };

        let lifecycle = awaiter.lifecycle_phase();

        debug_assert!(lifecycle != IDLE, "unregister called on an idle awaiter");

        // Notified awaiters were already removed by notify_one().
        if lifecycle != WAITING {
            return;
        }

        // SAFETY: Access is serialized by the caller's lock.
        let inner = unsafe { awaiter.inner_ref() };
        let next = inner.next;
        let prev = inner.prev;

        // SAFETY: The awaiter is in this set, so unlinking is safe.
        unsafe {
            self.unlink(prev, next);
        }

        // SAFETY: Access is serialized by the caller's lock.
        let inner = unsafe { awaiter.inner_mut() };
        *inner = Inner::idle();
        awaiter.set_lifecycle(IDLE, Ordering::Relaxed);
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
    // Mutating to return None causes all notification paths to hang
    // because waiters are never woken.
    #[cfg_attr(test, mutants::skip)]
    pub unsafe fn notify_one(&mut self) -> Option<Waker> {
        if self.head.is_null() {
            return None;
        }

        let ptr = self.pick_one();
        // SAFETY: ptr is a valid awaiter in the set.
        let awaiter = unsafe { &*ptr };
        // SAFETY: Access is serialized by the caller's lock.
        let inner = unsafe { awaiter.inner_ref() };
        let next = inner.next;
        let prev = inner.prev;

        // SAFETY: The awaiter is in this set, so unlinking is safe.
        unsafe {
            self.unlink(prev, next);
        }

        // Take the waker and clear the link fields before publishing
        // the notification via the atomic lifecycle store.
        // SAFETY: Access is serialized by the caller's lock.
        let inner = unsafe { awaiter.inner_mut() };
        let waker = inner.waker.take();
        inner.next = ptr::null_mut();
        inner.prev = ptr::null_mut();

        // Publish the notification. The Release ordering ensures all
        // prior writes on this thread (including data writes through
        // the UnsafeCell while the lock was held) are visible to the
        // polled task that observes this via take_notification().
        awaiter.set_lifecycle(NOTIFIED, Ordering::Release);

        waker
    }

    // Updates the head/tail and the neighbours' next/prev pointers to
    // splice out a single awaiter whose links were `prev` and `next`.
    //
    // # Safety
    //
    // `prev` and `next` must be the link fields of an awaiter currently
    // in this set. Access to all involved awaiters must be serialized.
    unsafe fn unlink(&mut self, prev: *mut Awaiter, next: *mut Awaiter) {
        if prev.is_null() {
            self.head = next;
        } else {
            // SAFETY: `prev` is a valid awaiter in the set.
            let prev = unsafe { &*prev };
            // SAFETY: Access is serialized by the caller's lock.
            unsafe { prev.inner_mut() }.next = next;
        }

        if next.is_null() {
            self.tail = prev;
        } else {
            // SAFETY: `next` is a valid awaiter in the set.
            let next = unsafe { &*next };
            // SAFETY: Access is serialized by the caller's lock.
            unsafe { next.inner_mut() }.prev = prev;
        }
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
}

impl Default for AwaiterSet {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: Raw pointers are `!Send` by default. This impl is sound
// because all pointer dereferences are serialized by external
// synchronization (Mutex or single-thread confinement). The set can
// safely be moved between threads as long as access remains exclusive.
unsafe impl Send for AwaiterSet {}

// AwaiterSet contains only raw pointers. All pointer dereferences are
// serialized by external synchronization, so no inconsistent state can
// be observed during unwind.
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
    fn new_set_is_empty() {
        let set = AwaiterSet::new();
        assert!(unsafe { set.is_empty() });
    }

    #[test]
    fn default_set_is_empty() {
        let set = AwaiterSet::default();
        assert!(unsafe { set.is_empty() });
    }

    #[test]
    fn notify_one_on_empty_returns_none() {
        let mut set = AwaiterSet::new();
        assert!(unsafe { set.notify_one() }.is_none());
    }

    #[test]
    fn register_and_notify_single_element() {
        let mut set = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), waker());
        }
        assert!(!unsafe { set.is_empty() });

        let waker = unsafe { set.notify_one() };
        assert!(waker.is_some());
        assert!(unsafe { set.is_empty() });
    }

    #[test]
    fn all_registered_awaiters_are_notified() {
        let mut set = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();
        let mut c = Awaiter::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), waker());
            set.register(Pin::new_unchecked(&mut b), waker());
            set.register(Pin::new_unchecked(&mut c), waker());
        }

        assert!(!unsafe { set.is_empty() });

        let mut notified = 0_usize;
        while let Some(w) = unsafe { set.notify_one() } {
            drop(w);
            notified = notified.checked_add(1).unwrap();
        }
        assert_eq!(notified, 3);

        for awaiter in [&a, &b, &c] {
            assert!(unsafe { Pin::new_unchecked(awaiter) }.is_notified());
        }

        assert!(unsafe { set.is_empty() });
    }

    #[test]
    fn remove_head_node() {
        let mut set = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), waker());
            set.register(Pin::new_unchecked(&mut b), waker());
            set.unregister(Pin::new_unchecked(&mut a));
        }

        assert!(!a.is_registered());
        let w = unsafe { set.notify_one() };
        assert!(w.is_some());
        assert!(unsafe { Pin::new_unchecked(&b) }.is_notified());
        assert!(unsafe { set.is_empty() });
    }

    #[test]
    fn remove_tail_node() {
        let mut set = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), waker());
            set.register(Pin::new_unchecked(&mut b), waker());
            set.unregister(Pin::new_unchecked(&mut b));
        }

        assert!(!b.is_registered());
        let w = unsafe { set.notify_one() };
        assert!(w.is_some());
        assert!(unsafe { Pin::new_unchecked(&a) }.is_notified());
        assert!(unsafe { set.is_empty() });
    }

    #[test]
    fn remove_middle_node() {
        let mut set = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();
        let mut c = Awaiter::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), waker());
            set.register(Pin::new_unchecked(&mut b), waker());
            set.register(Pin::new_unchecked(&mut c), waker());
            set.unregister(Pin::new_unchecked(&mut b));
        }

        assert!(!b.is_registered());
        drop(unsafe { set.notify_one() });
        drop(unsafe { set.notify_one() });
        assert!(unsafe { Pin::new_unchecked(&a) }.is_notified());
        assert!(unsafe { Pin::new_unchecked(&c) }.is_notified());
        assert!(unsafe { set.is_empty() });
    }

    #[test]
    fn remove_only_node() {
        let mut set = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), waker());
            set.unregister(Pin::new_unchecked(&mut a));
        }

        assert!(unsafe { set.is_empty() });
    }

    #[test]
    fn unregister_returns_to_idle() {
        let mut set = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), waker());
            set.register(Pin::new_unchecked(&mut b), waker());
            set.unregister(Pin::new_unchecked(&mut a));
        }

        assert!(!a.is_registered());
    }

    #[test]
    fn reuse_after_removal() {
        let mut set = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), waker());
            set.unregister(Pin::new_unchecked(&mut a));
            set.register(Pin::new_unchecked(&mut b), waker());
        }

        drop(unsafe { set.notify_one() });
        assert!(unsafe { Pin::new_unchecked(&b) }.is_notified());
        assert!(unsafe { set.is_empty() });
    }

    #[test]
    fn interleaved_register_and_notify() {
        let mut set = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();
        let mut c = Awaiter::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), waker());
            set.register(Pin::new_unchecked(&mut b), waker());
        }

        drop(unsafe { set.notify_one() });

        unsafe {
            set.register(Pin::new_unchecked(&mut c), waker());
        }

        drop(unsafe { set.notify_one() });
        drop(unsafe { set.notify_one() });
        assert!(unsafe { set.is_empty() });

        // Order is unspecified — but all three must have been notified.
        let count = [&a, &b, &c]
            .iter()
            .filter(|aw| unsafe { Pin::new_unchecked(**aw) }.is_notified())
            .count();
        assert_eq!(count, 3);
    }

    #[test]
    fn wakers_survive_notify() {
        let mut set = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), waker());
        }

        let recovered = unsafe { set.notify_one() };
        assert!(recovered.is_some());
    }

    #[test]
    fn notified_flag_survives_set_operations() {
        let mut set = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), waker());
        }

        drop(unsafe { set.notify_one() });
        assert!(unsafe { Pin::new_unchecked(&a) }.is_notified());
    }

    #[test]
    fn ten_elements_all_notified() {
        const ELEMENT_COUNT: usize = 10;
        let mut set = AwaiterSet::new();
        let mut nodes: Vec<Awaiter> = iter::repeat_with(Awaiter::new)
            .take(ELEMENT_COUNT)
            .collect();

        for node in &mut nodes {
            unsafe {
                set.register(Pin::new_unchecked(node), waker());
            }
        }

        for _ in 0..ELEMENT_COUNT {
            let w = unsafe { set.notify_one() };
            assert!(w.is_some());
        }

        assert!(unsafe { set.is_empty() });

        for node in &nodes {
            assert!(unsafe { Pin::new_unchecked(node) }.is_notified());
        }
    }

    #[test]
    fn debug_output_does_not_panic() {
        let set = AwaiterSet::new();
        let debug = format!("{set:?}");
        assert!(debug.contains("AwaiterSet"));
    }

    #[test]
    fn multithreaded_register_notify() {
        use std::sync::{Arc, Barrier, Mutex};
        use std::thread;

        struct Shared {
            set: AwaiterSet,
            notified: bool,
        }

        let shared = Arc::new(Mutex::new(Shared {
            set: AwaiterSet::new(),
            notified: false,
        }));
        let barrier = Arc::new(Barrier::new(2));

        let handle = thread::spawn({
            let shared = Arc::clone(&shared);
            let barrier = Arc::clone(&barrier);
            move || {
                let mut awaiter = Awaiter::new();

                {
                    let mut guard = shared.lock().unwrap();
                    unsafe {
                        guard
                            .set
                            .register(Pin::new_unchecked(&mut awaiter), Waker::noop().clone());
                    }
                }

                barrier.wait();
                barrier.wait();

                let guard = shared.lock().unwrap();
                assert!(guard.notified);
                assert!(unsafe { Pin::new_unchecked(&awaiter) }.is_notified());
            }
        });

        barrier.wait();
        {
            let mut guard = shared.lock().unwrap();
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
