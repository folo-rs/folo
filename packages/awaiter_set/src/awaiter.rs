use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomPinned;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::ptr;
use std::task::Waker;

use crate::AwaiterSet;

// The data that AwaiterSet accesses through stored raw pointers.
// Wrapped in UnsafeCell because the set stores *mut Awaiter pointers
// and later creates &mut references from them (via take_one/peek/
// for_each), bypassing the borrow checker. Without UnsafeCell,
// Miri's Stacked Borrows would flag these re-created references.
struct Inner {
    waker: Option<Waker>,
    notified: bool,
    user_data: usize,
    next: *mut Awaiter,
    prev: *mut Awaiter,
}

/// A single awaiter that can be registered in an [`AwaiterSet`].
///
/// Embed an `Awaiter` in your async future to park it until a
/// synchronization event occurs. The awaiter stores a [`Waker`] for
/// notification, a boolean notification flag, and a `usize` of
/// caller-defined data.
///
/// Once registered in a set, the awaiter must remain at a stable
/// pinned address until it is removed.
///
/// # Safety model
///
/// Methods that interact with the awaiter's internal state are
/// `unsafe` because the caller must guarantee exclusive access —
/// either by holding a mutex or by confining all access to a single
/// thread.
///
/// [`is_registered()`][Self::is_registered] is safe because it reads
/// a plain `bool` owned by the awaiter.
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

    /// Returns `true` if the awaiter is currently registered in an
    /// [`AwaiterSet`].
    #[must_use]
    pub fn is_registered(&self) -> bool {
        self.registered
    }

    /// Stores a waker and registers the awaiter in `set` if not
    /// already registered.
    ///
    /// On the first call this inserts the awaiter into the set and
    /// marks it as registered. On subsequent calls it only updates
    /// the stored waker.
    ///
    /// # Safety
    ///
    /// * The caller must have exclusive access to both the awaiter
    ///   and the set (e.g. by holding a lock).
    /// * The awaiter must be at a pinned, stable address.
    /// * The awaiter must remain valid until removed from the set.
    pub unsafe fn register(&mut self, set: &mut AwaiterSet, waker: Waker) {
        // SAFETY: The caller guarantees exclusive access to the awaiter.
        let inner = unsafe { &mut *self.inner.get() };
        inner.waker = Some(waker);
        if !self.registered {
            // SAFETY: Caller guarantees the awaiter is pinned and
            // will remain valid until removed.
            unsafe {
                set.insert(ptr::from_mut(self));
            }
            self.registered = true;
        }
    }

    /// Stores a waker with caller-defined data and registers the
    /// awaiter in `set` if not already registered.
    ///
    /// Behaves like [`register()`][Self::register] but also sets the
    /// [`user_data`][Self::user_data] (e.g. the number of permits a
    /// semaphore awaiter requests).
    ///
    /// # Safety
    ///
    /// Same requirements as [`register()`][Self::register].
    pub unsafe fn register_with_data(&mut self, set: &mut AwaiterSet, waker: Waker, data: usize) {
        // SAFETY: The caller guarantees exclusive access to the awaiter.
        let inner = unsafe { &mut *self.inner.get() };
        inner.waker = Some(waker);
        inner.user_data = data;
        if !self.registered {
            // SAFETY: Same as register().
            unsafe {
                set.insert(ptr::from_mut(self));
            }
            self.registered = true;
        }
    }

    /// Removes the awaiter from `set` if it is currently registered.
    ///
    /// # Safety
    ///
    /// * The caller must have exclusive access to both the awaiter
    ///   and the set.
    /// * The awaiter must be in `set` (not some other set).
    pub unsafe fn unregister(&mut self, set: &mut AwaiterSet) {
        if self.registered {
            // SAFETY: The awaiter is in this set per the caller's
            // contract.
            unsafe {
                set.remove(ptr::from_mut(self));
            }
            self.registered = false;
        }
    }

    /// Checks whether the awaiter was notified and, if so, clears
    /// the registration flag.
    ///
    /// Returns `true` if the awaiter has been notified, meaning the
    /// synchronization primitive has taken this awaiter from the set
    /// and transferred ownership of some resource (a lock, a permit,
    /// or a signal) to it.
    ///
    /// # Safety
    ///
    /// The caller must have exclusive access to the awaiter.
    pub unsafe fn take_notification(&mut self) -> bool {
        // SAFETY: The caller guarantees exclusive access to the awaiter.
        let inner = unsafe { &*self.inner.get() };
        if inner.notified {
            self.registered = false;
            true
        } else {
            false
        }
    }

    /// Checks whether the awaiter was notified, without changing
    /// registration state.
    ///
    /// Use this in drop handlers to decide whether to forward a
    /// resource to the next awaiter.
    ///
    /// # Safety
    ///
    /// The caller must have exclusive access to the awaiter.
    pub unsafe fn is_notified(&self) -> bool {
        // SAFETY: The caller guarantees exclusive access to the awaiter.
        let inner = unsafe { &*self.inner.get() };
        inner.notified
    }

    /// Marks this awaiter as notified.
    ///
    /// Called by synchronization primitives after taking the awaiter
    /// from the set, signaling the owning future to complete on its
    /// next poll.
    pub fn set_notified(&mut self) {
        // SAFETY: We have &mut self — exclusive access.
        let inner = unsafe { &mut *self.inner.get() };
        inner.notified = true;
    }

    /// Extracts and returns the stored waker, if any.
    ///
    /// Called by the synchronization primitive after taking this
    /// awaiter from the set and setting the notified flag. The caller
    /// must invoke [`Waker::wake()`] outside any lock scope to avoid
    /// re-entrancy issues.
    pub fn take_waker(&mut self) -> Option<Waker> {
        // SAFETY: We have &mut self — exclusive access.
        let inner = unsafe { &mut *self.inner.get() };
        inner.waker.take()
    }

    /// Returns the caller-defined user data.
    ///
    /// Defaults to `0` if never set.
    #[must_use]
    pub fn user_data(&self) -> usize {
        // SAFETY: We have &self — shared access is fine for reading.
        let inner = unsafe { &*self.inner.get() };
        inner.user_data
    }

    // Crate-internal accessors for AwaiterSet linked-list management.

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

        // SAFETY: Test has exclusive access.
        unsafe {
            a.register(&mut set, Waker::noop().clone());
        }
        assert!(a.is_registered());
        assert!(!set.is_empty());
    }

    #[test]
    fn register_idempotent_on_second_call() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            a.register(&mut set, Waker::noop().clone());
        }
        // SAFETY: Test has exclusive access.
        unsafe {
            a.register(&mut set, Waker::noop().clone());
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

        // SAFETY: Test has exclusive access.
        unsafe {
            a.register_with_data(&mut set, Waker::noop().clone(), 42);
        }
        assert!(a.is_registered());
        assert_eq!(a.user_data(), 42);
    }

    #[test]
    fn unregister_removes_from_set() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            a.register(&mut set, Waker::noop().clone());
        }
        // SAFETY: Test has exclusive access.
        unsafe {
            a.unregister(&mut set);
        }

        assert!(!a.is_registered());
        assert!(set.is_empty());
    }

    #[test]
    fn unregister_when_not_registered_is_noop() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            a.unregister(&mut set);
        }
        assert!(!a.is_registered());
    }

    #[test]
    fn take_notification_returns_false_when_not_notified() {
        let mut a = Awaiter::new();

        // SAFETY: Test has exclusive access.
        let notified = unsafe { a.take_notification() };
        assert!(!notified);
    }

    #[test]
    fn take_notification_returns_true_and_clears_registered() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            a.register(&mut set, Waker::noop().clone());
        }

        let node = set.take_one().unwrap();
        node.set_notified();

        assert!(a.is_registered());
        // SAFETY: Test has exclusive access.
        let notified = unsafe { a.take_notification() };
        assert!(notified);
        assert!(!a.is_registered());
    }

    #[test]
    fn is_notified_does_not_change_registered() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            a.register(&mut set, Waker::noop().clone());
        }
        let node = set.take_one().unwrap();
        node.set_notified();

        // SAFETY: Test has exclusive access.
        let notified = unsafe { a.is_notified() };
        assert!(notified);
        assert!(a.is_registered());
    }

    #[test]
    fn set_notified_and_take_waker() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            a.register(&mut set, Waker::noop().clone());
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

        // SAFETY: Test has exclusive access.
        unsafe {
            a.register(&mut set, Waker::noop().clone());
        }
        assert!(a.is_registered());

        let node = set.take_one().unwrap();
        node.set_notified();

        // SAFETY: Test has exclusive access.
        let notified = unsafe { a.take_notification() };
        assert!(notified);
        assert!(!a.is_registered());
    }

    #[test]
    fn full_lifecycle_register_unregister() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            a.register(&mut set, Waker::noop().clone());
        }
        assert!(a.is_registered());

        // SAFETY: Test has exclusive access.
        unsafe {
            a.unregister(&mut set);
        }
        assert!(!a.is_registered());
        assert!(set.is_empty());
    }
}
