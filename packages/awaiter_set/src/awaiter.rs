use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomPinned;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::ptr;
use std::task::Waker;

use crate::AwaiterSet;

// Interior data accessed by both the owning future (via &mut self
// methods) and by AwaiterSet (via stored raw pointers that are later
// turned into &mut Awaiter references). The UnsafeCell is required
// because the set bypasses the borrow checker by reconstructing
// references from raw pointers.
//
// Lifecycle of an Awaiter:
//
// 1. Idle: just created, no waker, not in any set.
// 2. Waiting: registered in a set via register(), waker stored.
//    The future's poll() calls register() on each poll to keep
//    the waker up to date (the executor may provide a different
//    waker on subsequent polls).
// 3. Notified: the synchronization primitive called take_one() on
//    the set, obtaining &mut Awaiter, then called set_notified()
//    and take_waker() to signal the future. The awaiter is no
//    longer in the set.
// 4. Back to Idle: the future's next poll observes the notification
//    via take_notification() and completes with Ready.
//
// Alternatively, the future may be dropped while in the Waiting
// state. The drop handler calls unregister() to remove the awaiter
// from the set, or (if already notified) forwards the resource
// to the next awaiter.
struct Inner {
    // The waker provided by the executor. Set during register(),
    // consumed by take_waker() after the primitive notifies us.
    // None when idle or after the waker has been taken.
    waker: Option<Waker>,

    // Set to true by the synchronization primitive (via
    // set_notified()) after taking this awaiter from the set.
    // Checked by the future's poll via take_notification().
    notified: bool,

    // Caller-defined metadata. Semaphores store the requested
    // permit count here; other primitives leave it at 0.
    user_data: usize,

    // Intrusive linked-set pointers, managed by AwaiterSet.
    next: *mut Awaiter,
    prev: *mut Awaiter,
}

/// An awaiter that can be registered in an [`AwaiterSet`].
///
/// Embed an `Awaiter` in your async future to park it until a
/// synchronization primitive signals it. When parked, the awaiter
/// holds a [`Waker`] that the primitive uses to wake the future.
///
/// An awaiter is removed from the set either by the primitive
/// (when it grants a resource like a lock or permit) or by the
/// future's drop handler (if the future is cancelled).
///
/// # Safety model
///
/// The `register`, `unregister`, `take_notification`, and
/// `is_notified` methods are `unsafe` because the awaiter's
/// internal state is shared with the [`AwaiterSet`] that manages
/// it. The caller must ensure that all access to the awaiter and
/// its set is serialized — for thread-safe primitives this means
/// holding the lock that protects the set; for single-threaded
/// primitives this is guaranteed by the `!Send` constraint.
pub struct Awaiter {
    inner: UnsafeCell<Inner>,
    registered: bool,
    _pinned: PhantomPinned,
}

impl Awaiter {
    /// Creates a new unregistered awaiter.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: UnsafeCell::new(Inner {
                waker: None,
                notified: false,
                user_data: 0,
                next: ptr::null_mut(),
                prev: ptr::null_mut(),
            }),
            registered: false,
            _pinned: PhantomPinned,
        }
    }

    /// Returns `true` if the awaiter is currently in an
    /// [`AwaiterSet`].
    #[must_use]
    pub fn is_registered(&self) -> bool {
        self.registered
    }

    /// Registers this awaiter in `set` with the given waker.
    ///
    /// On the first poll, this inserts the awaiter into the set. On
    /// subsequent polls it updates the stored waker (the executor
    /// may provide a different waker on each poll).
    ///
    /// # Safety
    ///
    /// The awaiter and the set must be protected by the same lock
    /// (or confined to a single thread). The awaiter must remain
    /// valid until it is removed from the set — either by the
    /// primitive calling [`AwaiterSet::take_one()`] or by the
    /// future's drop handler calling [`unregister()`][Self::unregister].
    pub unsafe fn register(self: Pin<&mut Self>, set: &mut AwaiterSet, waker: Waker) {
        // SAFETY: We do not move self. Pin guarantees address stability.
        let this = unsafe { self.get_unchecked_mut() };
        // SAFETY: Access is serialized by the caller's lock.
        let inner = unsafe { &mut *this.inner.get() };
        inner.waker = Some(waker);
        if !this.registered {
            inner.notified = false;
            inner.user_data = 0;
            // SAFETY: Pin guarantees the awaiter is at a stable
            // address and will remain valid until removed.
            unsafe {
                set.insert(ptr::from_mut(this));
            }
            this.registered = true;
        }
    }

    /// Registers this awaiter in `set` with a waker and
    /// caller-defined data.
    ///
    /// Behaves like [`register()`][Self::register] but also sets
    /// the [`user_data`][Self::user_data] value (e.g. the number
    /// of permits a semaphore awaiter requests).
    ///
    /// # Safety
    ///
    /// The awaiter and the set must be protected by the same lock
    /// (or confined to a single thread). The awaiter must remain
    /// valid until it is removed from the set.
    pub unsafe fn register_with_data(
        self: Pin<&mut Self>,
        set: &mut AwaiterSet,
        waker: Waker,
        data: usize,
    ) {
        // SAFETY: We do not move self.
        let this = unsafe { self.get_unchecked_mut() };
        // SAFETY: Access is serialized by the caller's lock.
        let inner = unsafe { &mut *this.inner.get() };
        inner.waker = Some(waker);
        inner.user_data = data;
        if !this.registered {
            inner.notified = false;
            // SAFETY: Same as register().
            unsafe {
                set.insert(ptr::from_mut(this));
            }
            this.registered = true;
        }
    }

    /// Removes the awaiter from `set`.
    ///
    /// This is a no-op if the awaiter is not currently registered.
    ///
    /// # Safety
    ///
    /// The awaiter and the set must be protected by the same lock
    /// (or confined to a single thread). If registered, the awaiter
    /// must be in `set` (not in a different set).
    pub unsafe fn unregister(self: Pin<&mut Self>, set: &mut AwaiterSet) {
        // SAFETY: We do not move self.
        let this = unsafe { self.get_unchecked_mut() };
        if this.registered {
            // SAFETY: The awaiter is in this set per the caller's
            // contract.
            unsafe {
                set.remove(ptr::from_mut(this));
            }
            this.registered = false;
        }
    }

    /// Checks whether the synchronization primitive has notified
    /// this awaiter, and if so, marks the awaiter as unregistered.
    ///
    /// Returns `true` if the primitive has called
    /// [`set_notified()`][Self::set_notified] on this awaiter
    /// (after taking it from the set via
    /// [`AwaiterSet::take_one()`]). The future should then complete
    /// with `Ready`.
    ///
    /// This is typically the first check in a future's `poll()`
    /// method.
    ///
    /// # Safety
    ///
    /// The awaiter must be protected by the same lock as the set
    /// (or confined to a single thread).
    #[must_use]
    pub unsafe fn take_notification(self: Pin<&mut Self>) -> bool {
        // SAFETY: We do not move self.
        let this = unsafe { self.get_unchecked_mut() };
        // SAFETY: Access is serialized by the caller's lock.
        let inner = unsafe { &*this.inner.get() };
        if inner.notified {
            this.registered = false;
            true
        } else {
            false
        }
    }

    /// Checks whether this awaiter has been notified, without
    /// changing its registration state.
    ///
    /// Used in the future's [`Drop`] implementation to determine
    /// whether the primitive granted a resource (lock, permit,
    /// signal) to this awaiter. If so, the drop handler must
    /// forward the resource to the next awaiter rather than
    /// silently discarding it.
    ///
    /// # Safety
    ///
    /// The awaiter must be protected by the same lock as the set
    /// (or confined to a single thread).
    #[must_use]
    pub unsafe fn is_notified(self: Pin<&Self>) -> bool {
        // SAFETY: Access is serialized by the caller's lock.
        let inner = unsafe { &*self.inner.get() };
        inner.notified
    }

    /// Marks this awaiter as notified.
    ///
    /// Called by synchronization primitives after taking the awaiter
    /// from the set via [`AwaiterSet::take_one()`], signaling the
    /// owning future to complete on its next poll.
    pub fn set_notified(&mut self) {
        // SAFETY: &mut self guarantees exclusive access.
        let inner = unsafe { &mut *self.inner.get() };
        inner.notified = true;
    }

    /// Extracts and returns the stored waker, if any.
    ///
    /// Called by the synchronization primitive after
    /// [`set_notified()`][Self::set_notified]. The caller must
    /// invoke [`Waker::wake()`] outside any lock scope to avoid
    /// re-entrancy issues.
    pub fn take_waker(&mut self) -> Option<Waker> {
        // SAFETY: &mut self guarantees exclusive access.
        let inner = unsafe { &mut *self.inner.get() };
        inner.waker.take()
    }

    /// Returns the caller-defined user data.
    ///
    /// Defaults to `0` if never set via
    /// [`register_with_data()`][Self::register_with_data].
    #[must_use]
    pub fn user_data(&self) -> usize {
        // SAFETY: &self is sufficient for reading through UnsafeCell
        // because the caller serializes all writes.
        let inner = unsafe { &*self.inner.get() };
        inner.user_data
    }

    // Crate-internal accessors for AwaiterSet linked-set management.

    pub(crate) fn next(&self) -> *mut Self {
        // SAFETY: Only called while holding exclusive access to the set.
        let inner = unsafe { &*self.inner.get() };
        inner.next
    }

    pub(crate) fn prev(&self) -> *mut Self {
        // SAFETY: Only called while holding exclusive access to the set.
        let inner = unsafe { &*self.inner.get() };
        inner.prev
    }

    pub(crate) fn set_next(&mut self, next: *mut Self) {
        // SAFETY: Only called while holding exclusive access to the set.
        let inner = unsafe { &mut *self.inner.get() };
        inner.next = next;
    }

    pub(crate) fn set_prev(&mut self, prev: *mut Self) {
        // SAFETY: Only called while holding exclusive access to the set.
        let inner = unsafe { &mut *self.inner.get() };
        inner.prev = prev;
    }
}

impl Default for Awaiter {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: The UnsafeCell contents are only accessed under external
// synchronization (Mutex or single-thread confinement). Sending the
// Awaiter to another thread is safe as long as access remains
// serialized.
unsafe impl Send for Awaiter {}

// Awaiter has no interior mutability visible to callers — all
// mutation requires &mut self or goes through unsafe methods with
// exclusive-access contracts.
impl UnwindSafe for Awaiter {}
impl RefUnwindSafe for Awaiter {}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for Awaiter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("registered", &self.registered)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::multiple_unsafe_ops_per_block,
    reason = "test code with trivial safety invariants"
)]
mod tests {
    use std::task::Waker;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(Awaiter: Send, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(Awaiter: Sync);

    #[test]
    fn new_awaiter_is_unregistered() {
        let a = Awaiter::new();
        assert!(!a.is_registered());
    }

    #[test]
    fn default_awaiter_is_unregistered() {
        let a = Awaiter::default();
        assert!(!a.is_registered());
    }

    #[test]
    fn register_sets_registered_flag() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access. The awaiter is not moved.
        unsafe {
            let a = Pin::new_unchecked(&mut a);
            a.register(&mut set, Waker::noop().clone());
        }
        assert!(a.is_registered());
        assert!(!set.is_empty());
    }

    #[test]
    fn register_idempotent_on_second_call() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access. The awaiter is not moved.
        unsafe {
            Pin::new_unchecked(&mut a).register(&mut set, Waker::noop().clone());
        }
        // SAFETY: Test has exclusive access. The awaiter is not moved.
        unsafe {
            Pin::new_unchecked(&mut a).register(&mut set, Waker::noop().clone());
        }

        assert!(a.is_registered());
        let popped = set.take_one();
        assert!(popped.is_some());
        assert!(set.is_empty());
    }

    #[test]
    fn register_with_data_stores_user_data() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access. The awaiter is not moved.
        unsafe {
            Pin::new_unchecked(&mut a).register_with_data(&mut set, Waker::noop().clone(), 42);
        }
        assert!(a.is_registered());
        assert_eq!(a.user_data(), 42);
    }

    #[test]
    fn unregister_removes_from_set() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access. The awaiter is not moved.
        unsafe {
            Pin::new_unchecked(&mut a).register(&mut set, Waker::noop().clone());
        }
        // SAFETY: Test has exclusive access. The awaiter is not moved.
        unsafe {
            Pin::new_unchecked(&mut a).unregister(&mut set);
        }

        assert!(!a.is_registered());
        assert!(set.is_empty());
    }

    #[test]
    fn unregister_when_not_registered_is_noop() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access. The awaiter is not moved.
        unsafe {
            Pin::new_unchecked(&mut a).unregister(&mut set);
        }
        assert!(!a.is_registered());
    }

    #[test]
    fn take_notification_returns_false_when_not_notified() {
        let mut a = Awaiter::new();

        // SAFETY: Test has exclusive access. The awaiter is not moved.
        let notified = unsafe { Pin::new_unchecked(&mut a).take_notification() };
        assert!(!notified);
    }

    #[test]
    fn take_notification_returns_true_and_clears_registered() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access. The awaiter is not moved.
        unsafe {
            Pin::new_unchecked(&mut a).register(&mut set, Waker::noop().clone());
        }

        let node = set.take_one().unwrap();
        node.set_notified();

        assert!(a.is_registered());
        // SAFETY: Test has exclusive access. The awaiter is not moved.
        let notified = unsafe { Pin::new_unchecked(&mut a).take_notification() };
        assert!(notified);
        assert!(!a.is_registered());
    }

    #[test]
    fn is_notified_does_not_change_registered() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access. The awaiter is not moved.
        unsafe {
            Pin::new_unchecked(&mut a).register(&mut set, Waker::noop().clone());
        }
        let node = set.take_one().unwrap();
        node.set_notified();

        // SAFETY: Test has exclusive access. The awaiter is not moved.
        let notified = unsafe { Pin::new_unchecked(&a).is_notified() };
        assert!(notified);
        assert!(a.is_registered());
    }

    #[test]
    fn set_notified_and_take_waker() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access. The awaiter is not moved.
        unsafe {
            Pin::new_unchecked(&mut a).register(&mut set, Waker::noop().clone());
        }

        let node = set.take_one().unwrap();
        node.set_notified();
        let waker = node.take_waker();
        assert!(waker.is_some());

        // Second take returns None.
        let waker2 = node.take_waker();
        assert!(waker2.is_none());
    }

    #[test]
    fn full_lifecycle_register_notify_take() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access. The awaiter is not moved.
        unsafe {
            Pin::new_unchecked(&mut a).register(&mut set, Waker::noop().clone());
        }
        assert!(a.is_registered());

        let node = set.take_one().unwrap();
        node.set_notified();

        // SAFETY: Test has exclusive access. The awaiter is not moved.
        let notified = unsafe { Pin::new_unchecked(&mut a).take_notification() };
        assert!(notified);
        assert!(!a.is_registered());
    }

    #[test]
    fn full_lifecycle_register_unregister() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access. The awaiter is not moved.
        unsafe {
            Pin::new_unchecked(&mut a).register(&mut set, Waker::noop().clone());
        }
        assert!(a.is_registered());

        // SAFETY: Test has exclusive access. The awaiter is not moved.
        unsafe {
            Pin::new_unchecked(&mut a).unregister(&mut set);
        }
        assert!(!a.is_registered());
        assert!(set.is_empty());
    }
}
