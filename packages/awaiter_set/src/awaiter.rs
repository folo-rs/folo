use std::any::type_name;
use std::cell::UnsafeCell;
use std::marker::PhantomPinned;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::sync::atomic::{AtomicU8, Ordering};
use std::task::Waker;
use std::{fmt, ptr};

// Lifecycle phase tracked by the atomic `lifecycle` field on Awaiter.
// Using an atomic outside UnsafeCell allows the poll path to check
// notification status without acquiring the protecting mutex.
pub(crate) const IDLE: u8 = 0;
pub(crate) const WAITING: u8 = 1;
pub(crate) const NOTIFIED: u8 = 2;

// Interior fields accessed by both the owning future (via Pin<&mut Self>
// methods) and by AwaiterSet (via stored raw pointers that are later
// turned into &mut Inner references). UnsafeCell is required because
// the set bypasses the borrow checker by reconstructing references from
// raw pointers — caller-supplied synchronization ensures all access is
// serialized.
//
// Data fields only — lifecycle tracking lives outside the UnsafeCell so
// it can be read without the protecting lock.
pub(crate) struct Inner {
    // Idle: None. Waiting: Some(waker). Notified: None (taken by
    // notify_one).
    pub(crate) waker: Option<Waker>,

    // Idle/Notified: null. Waiting: linked-set pointers maintained
    // by AwaiterSet.
    pub(crate) next: *mut Awaiter,
    pub(crate) prev: *mut Awaiter,
}

impl Inner {
    pub(crate) fn idle() -> Self {
        Self {
            waker: None,
            next: ptr::null_mut(),
            prev: ptr::null_mut(),
        }
    }
}

/// Represents a single waiting future in an [`AwaiterSet`][crate::AwaiterSet].
///
/// Embed an `Awaiter` in each async future that may need to wait for
/// a synchronization primitive. The primitive registers the awaiter
/// via [`AwaiterSet::register()`][crate::AwaiterSet::register] and later wakes it via
/// [`AwaiterSet::notify_one()`][crate::AwaiterSet::notify_one].
///
/// # Lifecycle
///
/// An awaiter goes through three phases:
///
/// 1. **Idle** — just created or after notification was consumed.
/// 2. **Waiting** — registered in a set, holding a waker.
/// 3. **Notified** — removed from the set by the primitive; the
///    owning future should complete with `Ready` on its next poll.
///
/// The lifecycle phase is tracked by an atomic field so that the
/// notification check ([`take_notification`][Self::take_notification])
/// can be performed without acquiring the protecting lock.
pub struct Awaiter {
    inner: UnsafeCell<Inner>,

    // Lifecycle phase (IDLE / WAITING / NOTIFIED). Lives outside the
    // UnsafeCell so it can be read atomically without the protecting
    // lock. The notifier stores NOTIFIED with Release after all
    // modifications to `inner`; the polled task loads with Acquire to
    // synchronize.
    lifecycle: AtomicU8,

    _pinned: PhantomPinned,
}

impl Awaiter {
    /// Creates a new awaiter in the idle state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: UnsafeCell::new(Inner::idle()),
            lifecycle: AtomicU8::new(IDLE),
            _pinned: PhantomPinned,
        }
    }

    /// Returns `true` if the awaiter is currently registered in a
    /// set or has been notified but not yet consumed.
    ///
    /// A future's [`Drop`] handler uses this to decide whether
    /// cleanup (unregistering or forwarding a resource) is needed.
    ///
    /// This reads the atomic lifecycle phase and does not require the
    /// protecting lock.
    #[must_use]
    pub fn is_registered(&self) -> bool {
        self.lifecycle.load(Ordering::Acquire) != IDLE
    }

    /// Consumes a pending notification, returning `true` if one
    /// was present.
    ///
    /// If the primitive has notified this awaiter (via
    /// [`AwaiterSet::notify_one()`][crate::AwaiterSet::notify_one]),
    /// this method returns `true` and transitions the awaiter back to
    /// the idle state. The future should then complete with
    /// `Poll::Ready`.
    ///
    /// This is typically the first check in a future's `poll()`.
    /// It reads the atomic lifecycle phase and does not require
    /// the protecting lock — the `Acquire` ordering synchronizes
    /// with the `Release` store in
    /// [`AwaiterSet::notify_one()`][crate::AwaiterSet::notify_one].
    #[must_use]
    pub fn take_notification(self: Pin<&Self>) -> bool {
        self.lifecycle
            .compare_exchange(NOTIFIED, IDLE, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }

    /// Returns `true` if this awaiter has been notified.
    ///
    /// Unlike [`take_notification()`][Self::take_notification], this
    /// does not consume the notification. Used in a future's
    /// [`Drop`] handler to decide whether the primitive granted a
    /// resource (lock, permit, signal) that must be forwarded to
    /// another awaiter instead of being silently discarded.
    ///
    /// This reads the atomic lifecycle phase and does not require
    /// the protecting lock.
    #[must_use]
    pub fn is_notified(self: Pin<&Self>) -> bool {
        self.lifecycle.load(Ordering::Acquire) == NOTIFIED
    }

    pub(crate) fn lifecycle_phase(&self) -> u8 {
        self.lifecycle.load(Ordering::Relaxed)
    }

    pub(crate) fn set_lifecycle(&self, phase: u8, ordering: Ordering) {
        self.lifecycle.store(phase, ordering);
    }

    /// Returns a mutable reference to the internal state.
    ///
    /// # Safety
    ///
    /// Access must be serialized by the caller (via a lock or
    /// single-thread confinement).
    #[expect(
        clippy::mut_from_ref,
        reason = "interior mutability via UnsafeCell is the \
                  intended pattern; caller serializes access"
    )]
    pub(crate) unsafe fn inner_mut(&self) -> &mut Inner {
        // SAFETY: Caller guarantees serialized access.
        unsafe { &mut *self.inner.get() }
    }

    /// Returns a shared reference to the internal state.
    ///
    /// # Safety
    ///
    /// Access must be serialized by the caller (via a lock or
    /// single-thread confinement).
    pub(crate) unsafe fn inner_ref(&self) -> &Inner {
        // SAFETY: Caller guarantees serialized access.
        unsafe { &*self.inner.get() }
    }
}

impl Default for Awaiter {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: The UnsafeCell contents are only accessed under external
// synchronization (Mutex or single-thread confinement). The AtomicU8
// lifecycle field is designed for concurrent access. Sending the
// Awaiter to another thread is safe as long as UnsafeCell access
// remains serialized.
unsafe impl Send for Awaiter {}

// Awaiter has no interior mutability visible to callers — all mutation
// requires &mut self or goes through unsafe methods with serialized-
// access contracts. The atomic lifecycle field is safely readable
// through shared references.
impl UnwindSafe for Awaiter {}
impl RefUnwindSafe for Awaiter {}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for Awaiter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let phase = match self.lifecycle.load(Ordering::Relaxed) {
            IDLE => "idle",
            WAITING => "waiting",
            NOTIFIED => "notified",
            _ => "unknown",
        };
        f.debug_struct(type_name::<Self>())
            .field("lifecycle", &phase)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block,
    reason = "Pin::new_unchecked + unsafe method calls are idiomatic in tests"
)]
mod tests {
    use std::task::Waker;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::AwaiterSet;

    assert_impl_all!(Awaiter: Send, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(Awaiter: Sync);

    #[test]
    fn new_awaiter_is_idle() {
        let a = Awaiter::new();
        assert!(!a.is_registered());
    }

    #[test]
    fn default_awaiter_is_idle() {
        let a = Awaiter::default();
        assert!(!a.is_registered());
    }

    #[test]
    fn register_transitions_to_waiting() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
        }
        assert!(a.is_registered());
        assert!(!unsafe { set.is_empty() });
    }

    #[test]
    fn re_register_replaces_waker() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
        }

        assert!(a.is_registered());
        let waker = unsafe { set.notify_one() };
        assert!(waker.is_some());
        assert!(unsafe { set.is_empty() });
    }

    #[test]
    fn unregister_transitions_to_idle() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
            set.unregister(Pin::new_unchecked(&mut a));
        }

        assert!(!a.is_registered());
        assert!(unsafe { set.is_empty() });
    }

    #[test]
    #[cfg_attr(debug_assertions, should_panic)]
    fn unregister_when_idle_panics_in_debug() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        unsafe {
            set.unregister(Pin::new_unchecked(&mut a));
        }
    }

    #[test]
    fn take_notification_returns_false_when_idle() {
        let a = Awaiter::new();

        let notified = unsafe { Pin::new_unchecked(&a) }.take_notification();
        assert!(!notified);
    }

    #[test]
    fn notify_transitions_to_notified() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
        }

        let node = unsafe { set.notify_one() };
        assert!(node.is_some());

        assert!(unsafe { Pin::new_unchecked(&a) }.is_notified());
    }

    #[test]
    fn take_notification_transitions_notified_to_idle() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
        }

        drop(unsafe { set.notify_one() });

        let notified = unsafe { Pin::new_unchecked(&a) }.take_notification();
        assert!(notified);
        assert!(!a.is_registered());
    }

    #[test]
    fn is_notified_does_not_change_state() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
        }
        drop(unsafe { set.notify_one() });

        assert!(unsafe { Pin::new_unchecked(&a) }.is_notified());
        assert!(unsafe { Pin::new_unchecked(&a) }.is_notified());
    }

    #[test]
    fn full_lifecycle_register_notify_take() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
        }
        assert!(a.is_registered());

        drop(unsafe { set.notify_one() });

        assert!(unsafe { Pin::new_unchecked(&a) }.take_notification());
        assert!(!a.is_registered());
    }

    #[test]
    fn full_lifecycle_register_unregister() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
        }
        assert!(a.is_registered());

        unsafe {
            set.unregister(Pin::new_unchecked(&mut a));
        }
        assert!(!a.is_registered());
        assert!(unsafe { set.is_empty() });
    }

    #[test]
    fn unregister_on_notified_is_noop() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
        }

        drop(unsafe { set.notify_one() });
        assert!(unsafe { Pin::new_unchecked(&a) }.is_notified());

        unsafe {
            set.unregister(Pin::new_unchecked(&mut a));
        }
        assert!(unsafe { Pin::new_unchecked(&a) }.is_notified());
    }

    #[test]
    fn re_registration_after_notification() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
        }
        drop(unsafe { set.notify_one() });
        assert!(unsafe { Pin::new_unchecked(&a) }.take_notification());
        assert!(!a.is_registered());

        unsafe {
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
        }
        assert!(a.is_registered());
        assert!(!unsafe { set.is_empty() });

        drop(unsafe { set.notify_one() });
    }

    #[test]
    fn take_notification_returns_false_for_waiting() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        unsafe {
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
        }

        assert!(!unsafe { Pin::new_unchecked(&a) }.take_notification());
        assert!(a.is_registered());
    }
}
