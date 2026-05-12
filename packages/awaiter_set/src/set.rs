use std::any::type_name;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
#[cfg(debug_assertions)]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::task::Waker;
use std::{fmt, ptr};

use crate::awaiter::{Awaiter, IDLE, Inner, NOTIFIED, WAITING};

// Monotonically increasing per-set identifier used to detect awaiters
// being passed to the wrong AwaiterSet (a class of corruption that the
// lifecycle field alone cannot catch). Zero is reserved for "not in any
// set", so we start at 1.
#[cfg(debug_assertions)]
static NEXT_SET_ID: AtomicU64 = AtomicU64::new(1);

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
/// # Reentrancy
///
/// Invoking a returned [`Waker`] may execute arbitrary user code which
/// in turn calls back into the owning primitive — dropping another
/// pending future, registering a new one, or signalling the primitive
/// again. To remain correct in the face of such reentrancy, code that
/// drains the set must always operate against the live, current set
/// rather than a borrowed snapshot of its contents.
///
/// The canonical drain loop:
///
/// 1. Take a short-lived borrow of the set.
/// 2. Call [`notify_one()`][Self::notify_one], stash the returned
///    waker, and release the borrow.
/// 3. Wake the future outside the borrow.
/// 4. Repeat from step 1 until [`notify_one()`][Self::notify_one]
///    returns `None`.
///
/// In particular, do not take a snapshot via [`std::mem::take`] or
/// [`std::mem::replace`] and drain that — a waker firing during the
/// drain may mutate the original (now-empty) set, leaving the
/// snapshot and the original out of sync. In debug builds an
/// owning-set invariant catches such cross-set operations.
///
/// See the canonical implementation in `events::ManualResetEvent::set`
/// for an example of this pattern.
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
///             if let Some(w) = guard.notify_one() {
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
///     // SAFETY: The awaiter is heap-pinned and remains valid
///     // until removed from the set.
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

    // Counter stamped onto each new registration via `Inner.epoch`.
    // Captured by `drain_threshold()` and consulted by
    // `notify_one_in_drain()` to skip awaiters registered during a
    // drain (e.g. by a reentrant waker firing mid-drain). The counter
    // is monotonically non-decreasing for the lifetime of the set;
    // because awaiters are always appended at the tail, the epoch
    // values stored in the list form a non-decreasing sequence from
    // head to tail. This lets `notify_one_in_drain` decide eligibility
    // by inspecting only the head.
    next_epoch: u64,

    // Debug-only sentinel used to detect awaiters being passed to a
    // different AwaiterSet than the one they are registered in (e.g.
    // via a snapshot/replace pattern that splits an awaiter from its
    // home set). Stored on each WAITING awaiter via `Inner.owning_set_id`.
    #[cfg(debug_assertions)]
    set_id: u64,
}

impl AwaiterSet {
    /// Creates a new empty set.
    #[must_use]
    pub fn new() -> Self {
        Self {
            head: ptr::null_mut(),
            tail: ptr::null_mut(),
            next_epoch: 1,
            #[cfg(debug_assertions)]
            set_id: NEXT_SET_ID.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// Returns `true` if the set contains no awaiters.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.head.is_null()
    }

    /// Registers an awaiter with the given waker.
    ///
    /// If the awaiter is idle, it is inserted into the set and the
    /// waker is stored. If the awaiter is already registered, only
    /// the stored waker is replaced.
    ///
    /// The awaiter must not be in the notified state. Call
    /// [`Awaiter::take_notification()`] to consume a pending
    /// notification before re-registering.
    ///
    /// # Safety
    ///
    /// The awaiter must remain pinned and valid until it is removed
    /// from the set (via [`unregister()`][Self::unregister] or
    /// [`notify_one()`][Self::notify_one]).
    pub unsafe fn register(&mut self, awaiter: Pin<&mut Awaiter>, waker: Waker) {
        // SAFETY: We do not move the awaiter. Pin guarantees address
        // stability.
        let awaiter = unsafe { awaiter.get_unchecked_mut() };

        debug_assert!(
            awaiter.lifecycle_phase() != NOTIFIED,
            "notified awaiters must consume the notification before re-registering",
        );

        if awaiter.lifecycle_phase() == WAITING {
            // SAFETY: Access is serialized by this `&mut self` lock scope; we hold
            // `&mut Awaiter` and no other `Inner` reference is live.
            let inner = unsafe { awaiter.inner_mut() };
            self.debug_assert_owns(inner, "register");
            // Already registered — update the waker in place.
            inner.waker = Some(waker);
            return;
        }

        // New registration — initialize fields and insert.
        // SAFETY: Access is serialized by this `&mut self` lock scope; we hold
        // `&mut Awaiter` and no other `Inner` reference is live.
        let inner = unsafe { awaiter.inner_mut() };
        inner.waker = Some(waker);
        inner.next = ptr::null_mut();
        inner.prev = self.tail;
        inner.epoch = self.next_epoch;
        #[cfg(debug_assertions)]
        {
            inner.owning_set_id = self.set_id;
        }

        let ptr = ptr::from_mut(awaiter);

        if self.tail.is_null() {
            self.head = ptr;
        } else {
            // SAFETY: Validity — `self.tail` is non-null (just checked) and the set
            // invariant guarantees it points to an `Awaiter` currently registered in
            // this set; per `register`'s safety contract, registered awaiters remain
            // pinned and valid until removed. Aliasing — `Awaiter`'s public API is
            // `&self`-only; access to its `Inner` is gated by this `&mut self` lock
            // scope; and the tail awaiter is a distinct object from `awaiter` (which
            // is being newly inserted), so the existing `&mut Awaiter` borrow does
            // not alias.
            let tail = unsafe { &*self.tail };
            // SAFETY: Access is serialized by the caller's lock; no other reference
            // to `tail`'s `Inner` is live (we hold only `&Awaiter` to it here).
            unsafe { tail.inner_mut() }.next = ptr;
        }
        self.tail = ptr;

        awaiter.set_lifecycle(WAITING, Ordering::Release);
    }

    /// Removes an awaiter from the set, returning it to the idle
    /// state.
    ///
    /// If the awaiter has already been notified (and therefore
    /// already removed from the set by
    /// [`notify_one()`][Self::notify_one]), this is a no-op.
    ///
    /// # Safety
    ///
    /// The awaiter must currently be registered with this set (or
    /// already notified by it).
    pub unsafe fn unregister(&mut self, awaiter: Pin<&mut Awaiter>) {
        // SAFETY: We do not move the awaiter.
        let awaiter = unsafe { awaiter.get_unchecked_mut() };

        let lifecycle = awaiter.lifecycle_phase();

        debug_assert!(lifecycle != IDLE, "unregister called on an idle awaiter");

        // Notified awaiters were already removed by notify_one().
        if lifecycle != WAITING {
            return;
        }

        // SAFETY: Access is serialized by this `&mut self` lock scope; we hold
        // `&mut Awaiter` and no other `Inner` reference is live.
        let inner = unsafe { awaiter.inner_ref() };
        self.debug_assert_owns(inner, "unregister");
        let next = inner.next;
        let prev = inner.prev;

        // SAFETY: The awaiter is in this set, so unlinking is safe.
        unsafe {
            self.unlink(prev, next);
        }

        // SAFETY: Access is serialized by this `&mut self` lock scope; the previous
        // `inner_ref` borrow has ended (its values were copied into `next`/`prev`),
        // so no other `Inner` reference is live.
        let inner = unsafe { awaiter.inner_mut() };
        *inner = Inner::idle();
        awaiter.set_lifecycle(IDLE, Ordering::Release);
    }

    /// Removes one awaiter and returns its waker.
    ///
    /// The awaiter transitions to the notified state. The future
    /// that owns it will observe this on its next poll via
    /// [`Awaiter::take_notification()`] and complete with `Ready`.
    ///
    /// The caller may want to invoke [`Waker::wake()`] outside any lock
    /// scope to prevent reentrancy deadlocks.
    ///
    /// Returns `None` if the set is empty.
    pub fn notify_one(&mut self) -> Option<Waker> {
        if self.head.is_null() {
            return None;
        }

        let ptr = self.pick_one();
        // SAFETY: Validity — `pick_one` returns a non-null pointer (we already
        // checked `self.head.is_null()` above) drawn from the set's link fields,
        // which the set invariant requires point to awaiters currently registered;
        // per `register`'s safety contract, those awaiters remain pinned and valid
        // until removed. Aliasing — the dereference happens only inside `remove`,
        // which is the sole entry point for any further access to this awaiter.
        Some(unsafe { self.remove(ptr) })
    }

    /// Returns an epoch threshold for the current drain.
    ///
    /// Use with [`notify_one_in_drain()`][Self::notify_one_in_drain]
    /// to drain only awaiters that were already registered when this
    /// method was called. Awaiters registered after the returned
    /// threshold was captured — typically by a reentrant waker
    /// firing mid-drain — will be skipped.
    ///
    /// Each call advances an internal counter so threshold values
    /// from sequential drains do not collide.
    pub fn drain_threshold(&mut self) -> u64 {
        let threshold = self.next_epoch;
        self.next_epoch = self.next_epoch.wrapping_add(1);
        threshold
    }

    /// Like [`notify_one()`][Self::notify_one], but only picks an
    /// awaiter whose registration epoch is at or before `threshold`.
    ///
    /// Returns `None` when no eligible awaiter exists — either the
    /// set is empty, or every remaining awaiter was registered after
    /// the threshold was captured via
    /// [`drain_threshold()`][Self::drain_threshold].
    ///
    /// The set's internal list is ordered head-to-tail by registration
    /// time, so the head always carries the smallest epoch value. The
    /// drain therefore proceeds in strict FIFO order regardless of the
    /// debug-build ordering of [`notify_one()`][Self::notify_one].
    pub fn notify_one_in_drain(&mut self, threshold: u64) -> Option<Waker> {
        if self.head.is_null() {
            return None;
        }

        // SAFETY: Validity — `self.head` is non-null (just checked) and the set
        // invariant guarantees it points to an `Awaiter` currently registered in
        // this set; per `register`'s safety contract, registered awaiters remain
        // pinned and valid until removed. Aliasing — `Awaiter`'s public API is
        // `&self`-only; access to its `Inner` is gated by this `&mut self` lock
        // scope; the owning future is asleep (it is a registered waiter), so no
        // `&mut Awaiter` to it is live elsewhere.
        let head = unsafe { &*self.head };
        // SAFETY: Access is serialized by this `&mut self` lock scope; we hold
        // only `&Awaiter` and no `&mut Inner` is live.
        let head_epoch = unsafe { head.inner_ref() }.epoch;

        if head_epoch > threshold {
            return None;
        }

        // SAFETY: Same as above for `self.head`.
        Some(unsafe { self.remove(self.head) })
    }

    /// Unlinks the awaiter at `ptr` from the set and returns its waker.
    ///
    /// # Safety
    ///
    /// `ptr` must point to an awaiter currently registered in this
    /// set. The caller's lock must guard concurrent access.
    unsafe fn remove(&mut self, ptr: *mut Awaiter) -> Waker {
        // SAFETY: Per the function's safety contract, `ptr` references a pinned,
        // valid `Awaiter` currently in the set. Aliasing — `Awaiter`'s public API
        // is `&self`-only; access to its `Inner` is gated by this `&mut self` lock
        // scope; the owning future is asleep, so no `&mut Awaiter` to it is live.
        let awaiter = unsafe { &*ptr };
        // SAFETY: Access is serialized by this `&mut self` lock scope; we hold only
        // `&Awaiter` and no `&mut Inner` is live.
        let inner = unsafe { awaiter.inner_ref() };
        self.debug_assert_owns(inner, "notify_one");
        let next = inner.next;
        let prev = inner.prev;

        // SAFETY: The awaiter is in this set (per this function's safety contract),
        // so unlinking is safe.
        unsafe {
            self.unlink(prev, next);
        }

        // Take the waker and clear the link fields before publishing
        // the notification via the atomic lifecycle store.
        // SAFETY: Access is serialized by this `&mut self` lock scope; the previous
        // `inner_ref` borrow has ended (its values were copied into `next`/`prev`),
        // so no other `Inner` reference is live.
        let inner = unsafe { awaiter.inner_mut() };
        let waker = inner.waker.take();
        inner.next = ptr::null_mut();
        inner.prev = ptr::null_mut();
        inner.epoch = 0;
        #[cfg(debug_assertions)]
        {
            inner.owning_set_id = 0;
        }

        // Publish the notification. The Release ordering ensures all
        // prior writes on this thread (including data writes through
        // the UnsafeCell while the lock was held) are visible to the
        // polled task that observes this via take_notification().
        awaiter.set_lifecycle(NOTIFIED, Ordering::Release);

        waker.expect("waker is always set while the awaiter is in WAITING state")
    }

    // Verifies in debug builds that the awaiter referenced by `inner`
    // is registered in this set, not a different one. Catches bugs
    // where an awaiter's home set has been split from `self` (e.g. via
    // a snapshot/replace pattern) and the wrong set is being mutated.
    #[cfg(debug_assertions)]
    fn debug_assert_owns(&self, inner: &Inner, op: &str) {
        debug_assert!(
            inner.owning_set_id == self.set_id,
            "AwaiterSet::{op} called on an awaiter that belongs to a different set \
             (awaiter.owning_set_id = {}, self.set_id = {})",
            inner.owning_set_id,
            self.set_id,
        );
    }

    #[cfg(not(debug_assertions))]
    #[expect(
        clippy::unused_self,
        reason = "signature matches the debug build for call-site uniformity"
    )]
    #[inline(always)]
    fn debug_assert_owns(&self, _inner: &Inner, _op: &str) {}

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
            // SAFETY: Validity — `prev` is a link field of an awaiter currently in
            // this set (per `unlink`'s safety contract), so it points to a pinned,
            // valid `Awaiter`. Aliasing — `Awaiter`'s public API is `&self`-only;
            // access to its `Inner` is gated by this `&mut self` lock scope; and
            // `prev` is a distinct awaiter from the one being unlinked, so no
            // `&mut Awaiter` borrow held elsewhere in this call aliases.
            let prev = unsafe { &*prev };
            // SAFETY: Access is serialized by the caller's lock; no other reference
            // to `prev`'s `Inner` is live (we hold only `&Awaiter` to it here).
            unsafe { prev.inner_mut() }.next = next;
        }

        if next.is_null() {
            self.tail = prev;
        } else {
            // SAFETY: Validity — `next` is a link field of an awaiter currently in
            // this set (per `unlink`'s safety contract), so it points to a pinned,
            // valid `Awaiter`. Aliasing — `Awaiter`'s public API is `&self`-only;
            // access to its `Inner` is gated by this `&mut self` lock scope; and
            // `next` is a distinct awaiter from the one being unlinked, so no
            // `&mut Awaiter` borrow held elsewhere in this call aliases.
            let next = unsafe { &*next };
            // SAFETY: Access is serialized by the caller's lock; no other reference
            // to `next`'s `Inner` is live (we hold only `&Awaiter` to it here).
            unsafe { next.inner_mut() }.prev = prev;
        }
    }

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
        assert!(set.is_empty());
    }

    #[test]
    fn default_set_is_empty() {
        let set = AwaiterSet::default();
        assert!(set.is_empty());
    }

    #[test]
    fn notify_one_on_empty_returns_none() {
        let mut set = AwaiterSet::new();
        assert!(set.notify_one().is_none());
    }

    #[test]
    fn register_and_notify_single_element() {
        let mut set = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), waker());
        }
        assert!(!set.is_empty());

        let waker = set.notify_one();
        assert!(waker.is_some());
        assert!(set.is_empty());
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

        assert!(!set.is_empty());

        let mut notified = 0_usize;
        while let Some(w) = set.notify_one() {
            drop(w);
            notified = notified.checked_add(1).unwrap();
        }
        assert_eq!(notified, 3);

        for awaiter in [&a, &b, &c] {
            assert!(awaiter.is_notified());
        }

        assert!(set.is_empty());
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
        let w = set.notify_one();
        assert!(w.is_some());
        assert!(b.is_notified());
        assert!(set.is_empty());
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
        let w = set.notify_one();
        assert!(w.is_some());
        assert!(a.is_notified());
        assert!(set.is_empty());
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
        drop(set.notify_one());
        drop(set.notify_one());
        assert!(a.is_notified());
        assert!(c.is_notified());
        assert!(set.is_empty());
    }

    #[test]
    fn remove_only_node() {
        let mut set = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), waker());
            set.unregister(Pin::new_unchecked(&mut a));
        }

        assert!(set.is_empty());
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

        drop(set.notify_one());
        assert!(b.is_notified());
        assert!(set.is_empty());
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

        drop(set.notify_one());

        unsafe {
            set.register(Pin::new_unchecked(&mut c), waker());
        }

        drop(set.notify_one());
        drop(set.notify_one());
        assert!(set.is_empty());

        // Order is unspecified — but all three must have been notified.
        let count = [&a, &b, &c].iter().filter(|aw| aw.is_notified()).count();
        assert_eq!(count, 3);
    }

    #[test]
    fn wakers_survive_notify() {
        let mut set = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), waker());
        }

        let recovered = set.notify_one();
        assert!(recovered.is_some());
    }

    #[test]
    fn notified_flag_survives_set_operations() {
        let mut set = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), waker());
        }

        drop(set.notify_one());
        assert!(a.is_notified());
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
            let w = set.notify_one();
            assert!(w.is_some());
        }

        assert!(set.is_empty());

        for node in &nodes {
            assert!(node.is_notified());
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
                assert!(awaiter.is_notified());
            }
        });

        barrier.wait();
        {
            let mut guard = shared.lock().unwrap();
            let waker = guard.set.notify_one();
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
                    if let Some(w) = guard.notify_one() {
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

    // The owning-set-id invariant is only present in debug builds.
    // The panic-tests below follow the repo convention: the function
    // call is sound to perform in release (where the assert is
    // stripped), but in debug builds it must trip the invariant.

    #[test]
    #[cfg_attr(debug_assertions, should_panic)]
    fn unregister_with_wrong_set_panics_in_debug() {
        let mut set_a = AwaiterSet::new();
        let mut set_b = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            set_a.register(Pin::new_unchecked(&mut a), waker());
        }

        unsafe {
            set_b.unregister(Pin::new_unchecked(&mut a));
        }
    }

    #[test]
    #[cfg_attr(debug_assertions, should_panic)]
    fn register_in_second_set_while_waiting_in_first_panics_in_debug() {
        let mut set_a = AwaiterSet::new();
        let mut set_b = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            set_a.register(Pin::new_unchecked(&mut a), waker());
        }

        unsafe {
            set_b.register(Pin::new_unchecked(&mut a), waker());
        }
    }

    #[test]
    #[cfg_attr(debug_assertions, should_panic)]
    fn unregister_after_snapshot_panics_in_debug() {
        // Reproduces the corruption pattern from PR 141: an awaiter is
        // registered in a set that is then moved out via mem::take. The
        // original location gets a fresh set_id. Calling unregister on
        // the original (post-take) for an awaiter that lives in the
        // snapshot must trip the invariant — this is the assertion
        // that catches snapshot/replace-style state corruption.
        let mut original = AwaiterSet::new();
        let mut a = Awaiter::new();

        unsafe {
            original.register(Pin::new_unchecked(&mut a), waker());
        }

        // Move the live state into a snapshot; the original is now a
        // fresh, empty set with a different set_id.
        let _snapshot = std::mem::take(&mut original);

        // A still has owning_set_id equal to the OLD set_id, which now
        // lives only in `_snapshot`. The fresh `original` does not own
        // this awaiter.
        unsafe {
            original.unregister(Pin::new_unchecked(&mut a));
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn each_set_has_a_unique_id() {
        // The owning-set-id mechanism relies on freshly constructed
        // sets having distinct ids. We do not pin the exact values
        // (they monotonically increase via a static counter that may
        // already be advanced by other tests), only that no two new
        // sets share the same id.
        let a = AwaiterSet::new();
        let b = AwaiterSet::new();
        let c = AwaiterSet::default();
        assert_ne!(a.set_id, b.set_id);
        assert_ne!(b.set_id, c.set_id);
        assert_ne!(a.set_id, c.set_id);
    }

    #[test]
    fn drain_threshold_advances_per_call() {
        let mut set = AwaiterSet::new();
        let first = set.drain_threshold();
        let second = set.drain_threshold();
        let third = set.drain_threshold();
        assert!(second > first);
        assert!(third > second);
    }

    #[test]
    fn notify_one_in_drain_skips_awaiter_registered_after_threshold() {
        let mut set = AwaiterSet::new();
        let mut early = Awaiter::new();
        unsafe {
            set.register(Pin::new_unchecked(&mut early), waker());
        }

        let threshold = set.drain_threshold();

        // Awaiter registered after the threshold capture — must be
        // skipped by `notify_one_in_drain(threshold)`.
        let mut late = Awaiter::new();
        unsafe {
            set.register(Pin::new_unchecked(&mut late), waker());
        }

        // First call drains the early awaiter.
        let w = set.notify_one_in_drain(threshold);
        assert!(w.is_some());
        assert!(early.is_notified());
        assert!(!late.is_notified());

        // Second call — only the late awaiter is left, and it has an
        // epoch beyond the threshold, so the drain finishes.
        let w = set.notify_one_in_drain(threshold);
        assert!(w.is_none());
        assert!(!late.is_notified());
        assert!(!set.is_empty());

        // Clean up the late awaiter — required because Awaiter::drop
        // asserts the awaiter is not still registered.
        unsafe {
            set.unregister(Pin::new_unchecked(&mut late));
        }
    }

    #[test]
    fn notify_one_in_drain_on_empty_returns_none() {
        let mut set = AwaiterSet::new();
        let threshold = set.drain_threshold();
        assert!(set.notify_one_in_drain(threshold).is_none());
    }

    #[test]
    fn notify_one_in_drain_drains_all_prior_awaiters_in_fifo_order() {
        let mut set = AwaiterSet::new();
        let mut a = Awaiter::new();
        let mut b = Awaiter::new();
        let mut c = Awaiter::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), waker());
            set.register(Pin::new_unchecked(&mut b), waker());
            set.register(Pin::new_unchecked(&mut c), waker());
        }

        let threshold = set.drain_threshold();

        let mut drained = 0_usize;
        while let Some(w) = set.notify_one_in_drain(threshold) {
            drop(w);
            drained = drained.checked_add(1).unwrap();
        }

        assert_eq!(drained, 3);
        for awaiter in [&a, &b, &c] {
            assert!(awaiter.is_notified());
        }
        assert!(set.is_empty());
    }

    #[test]
    fn successive_drain_batches_use_independent_thresholds() {
        let mut set = AwaiterSet::new();

        let mut first_batch = Awaiter::new();
        unsafe {
            set.register(Pin::new_unchecked(&mut first_batch), waker());
        }
        let threshold_a = set.drain_threshold();
        drop(set.notify_one_in_drain(threshold_a));
        assert!(first_batch.is_notified());

        let mut second_batch = Awaiter::new();
        unsafe {
            set.register(Pin::new_unchecked(&mut second_batch), waker());
        }
        let threshold_b = set.drain_threshold();
        assert!(threshold_b > threshold_a);
        drop(set.notify_one_in_drain(threshold_b));
        assert!(second_batch.is_notified());
        assert!(set.is_empty());
    }
}
