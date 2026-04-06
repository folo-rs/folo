use std::cell::UnsafeCell;
use std::marker::PhantomPinned;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::task::Waker;
use std::{fmt, ptr};

use crate::AwaiterSet;

// Interior state accessed by both the owning future (via Pin<&mut Self>
// methods) and by AwaiterSet (via stored raw pointers that are later
// turned into &mut Awaiter references). The UnsafeCell is required
// because the set bypasses the borrow checker by reconstructing
// references from raw pointers.
//
// Lifecycle: Idle -> Waiting -> Notified -> Idle.
//
// The Waiting state holds the waker, linked-set pointers, and
// optional user data. After the primitive takes the awaiter from
// the set via take_one(), it transitions to Notified (which
// preserves user_data for the drop handler). The future's next
// poll observes the notification and returns Ready.
#[expect(
    variant_size_differences,
    reason = "Waiting is the hot path; boxing would add overhead"
)]
enum State {
    /// Just created or returned to idle after notification was
    /// consumed. Not in any set.
    Idle,

    /// Registered in a set. Holds the waker for async notification
    /// and the intrusive linked-set pointers.
    Waiting {
        waker: Option<Waker>,
        user_data: usize,
        next: *mut Awaiter,
        prev: *mut Awaiter,
    },

    /// Taken from the set by the synchronization primitive.
    /// The owning future will observe this on the next poll and
    /// complete with Ready.
    Notified { user_data: usize },
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
    state: UnsafeCell<State>,
    _pinned: PhantomPinned,
}

impl Awaiter {
    /// Creates a new awaiter in the Idle state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: UnsafeCell::new(State::Idle),
            _pinned: PhantomPinned,
        }
    }

    /// Returns `true` if the awaiter is in an active lifecycle
    /// (Waiting or Notified). Drop handlers use this to decide
    /// whether cleanup (unregistering or forwarding) is needed.
    #[must_use]
    pub fn is_registered(&self) -> bool {
        // SAFETY: Reading the discriminant does not race because
        // the owning future holds &self.
        !matches!(unsafe { &*self.state.get() }, State::Idle)
    }

    /// Registers this awaiter in `set` with the given waker.
    ///
    /// Transitions from Idle to Waiting.
    ///
    /// # Panics
    ///
    /// Panics if the awaiter is not in the Idle state.
    ///
    /// # Safety
    ///
    /// The awaiter and the set must be protected by the same lock
    /// (or confined to a single thread). The awaiter must remain
    /// valid until it is removed from the set — either by the
    /// primitive calling [`AwaiterSet::take_one()`] or by the
    /// future's drop handler calling [`unregister()`][Self::unregister].
    pub unsafe fn register(self: Pin<&mut Self>, set: &mut AwaiterSet, waker: Waker) {
        // SAFETY: Caller ensures the same safety requirements.
        unsafe { self.register_with_data(set, waker, 0) }
    }

    /// Registers this awaiter in `set` with a waker and
    /// caller-defined data.
    ///
    /// Behaves like [`register()`][Self::register] but also sets
    /// the [`user_data`][Self::user_data] value (e.g. the number
    /// of permits a semaphore awaiter requests).
    ///
    /// # Panics
    ///
    /// Panics if the awaiter is not in the Idle state.
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
        assert!(
            // SAFETY: Access is serialized by the caller's lock.
            matches!(unsafe { &*this.state.get() }, State::Idle),
            "register called on non-idle awaiter"
        );
        // SAFETY: Access is serialized by the caller's lock.
        unsafe {
            *this.state.get() = State::Waiting {
                waker: Some(waker),
                user_data: data,
                next: ptr::null_mut(),
                prev: ptr::null_mut(),
            };
        }
        // SAFETY: Same as register().
        unsafe {
            set.insert(ptr::from_mut(this));
        }
    }

    /// Updates the stored waker for a registered awaiter.
    ///
    /// Called on subsequent polls after the initial
    /// [`register()`][Self::register] to keep the waker current
    /// (the executor may provide a different waker on each poll).
    ///
    /// # Safety
    ///
    /// The awaiter must be protected by the same lock as its set
    /// (or confined to a single thread).
    pub unsafe fn update_waker(self: Pin<&mut Self>, new_waker: Waker) {
        // SAFETY: We do not move self.
        let this = unsafe { self.get_unchecked_mut() };
        // SAFETY: Access is serialized by the caller's lock.
        let state = unsafe { &mut *this.state.get() };
        match state {
            State::Waiting { waker, .. } => *waker = Some(new_waker),
            _ => debug_assert!(false, "update_waker called on non-waiting awaiter"),
        }
    }

    /// Removes the awaiter from `set`, transitioning to Idle.
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
        // SAFETY: Access is serialized by the caller's lock.
        if matches!(unsafe { &*this.state.get() }, State::Waiting { .. }) {
            // SAFETY: The awaiter is in this set per the caller's
            // contract.
            unsafe {
                set.remove(ptr::from_mut(this));
            }
            // SAFETY: Access is serialized by the caller's lock.
            unsafe {
                *this.state.get() = State::Idle;
            }
        }
    }

    /// Checks whether the synchronization primitive has notified
    /// this awaiter, and if so, transitions back to Idle.
    ///
    /// Returns `true` if the awaiter is in the Notified state
    /// (meaning the primitive took it from the set and called
    /// [`notify()`][Self::notify]). The future should then complete
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
        if matches!(unsafe { &*this.state.get() }, State::Notified { .. }) {
            // SAFETY: Access is serialized by the caller's lock.
            unsafe {
                *this.state.get() = State::Idle;
            }
            true
        } else {
            false
        }
    }

    /// Checks whether this awaiter has been notified, without
    /// changing its state.
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
        matches!(unsafe { &*self.state.get() }, State::Notified { .. })
    }

    /// Notifies this awaiter, consuming its waker.
    ///
    /// Called by synchronization primitives after taking the awaiter
    /// from the set via [`AwaiterSet::take_one()`]. Transitions the
    /// awaiter from Waiting to Notified and returns the stored waker
    /// so the primitive can call [`Waker::wake()`] outside any lock
    /// scope to avoid re-entrancy issues.
    pub(crate) fn notify(&mut self) -> Option<Waker> {
        // SAFETY: &mut self guarantees exclusive access.
        let state = unsafe { &mut *self.state.get() };
        match std::mem::replace(state, State::Idle) {
            State::Waiting {
                waker, user_data, ..
            } => {
                *state = State::Notified { user_data };
                waker
            }
            other => {
                // Not in Waiting state — restore and return None.
                *state = other;
                None
            }
        }
    }

    /// Extracts the waker without changing the notification state.
    #[cfg(test)]
    pub(crate) fn take_waker(&mut self) -> Option<Waker> {
        // SAFETY: &mut self guarantees exclusive access.
        let state = unsafe { &mut *self.state.get() };
        if let State::Waiting { waker, .. } = state {
            waker.take()
        } else {
            None
        }
    }

    /// Returns the caller-defined user data.
    ///
    /// Returns `0` for awaiters in the Idle state.
    #[must_use]
    pub fn user_data(&self) -> usize {
        // SAFETY: &self is sufficient for reading.
        match unsafe { &*self.state.get() } {
            State::Idle => 0,
            State::Waiting { user_data, .. } | State::Notified { user_data } => *user_data,
        }
    }

    // Crate-internal accessors for AwaiterSet linked-set management.

    pub(crate) fn next(&self) -> *mut Self {
        // SAFETY: Only called on awaiters in the Waiting state.
        match unsafe { &*self.state.get() } {
            State::Waiting { next, .. } => *next,
            _ => ptr::null_mut(),
        }
    }

    pub(crate) fn prev(&self) -> *mut Self {
        // SAFETY: Only called on awaiters in the Waiting state.
        match unsafe { &*self.state.get() } {
            State::Waiting { prev, .. } => *prev,
            _ => ptr::null_mut(),
        }
    }

    pub(crate) fn set_next(&mut self, new_next: *mut Self) {
        // SAFETY: Only called on awaiters in the Waiting state.
        let state = unsafe { &mut *self.state.get() };
        if let State::Waiting { next, .. } = state {
            *next = new_next;
        }
    }

    pub(crate) fn set_prev(&mut self, new_prev: *mut Self) {
        // SAFETY: Only called on awaiters in the Waiting state.
        let state = unsafe { &mut *self.state.get() };
        if let State::Waiting { prev, .. } = state {
            *prev = new_prev;
        }
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
// serialized-access contracts.
impl UnwindSafe for Awaiter {}
impl RefUnwindSafe for Awaiter {}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for Awaiter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("registered", &self.is_registered())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::multiple_unsafe_ops_per_block,
    reason = "Pin::new_unchecked + unsafe method calls are idiomatic in tests"
)]
mod tests {
    use std::task::Waker;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

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

        // SAFETY: Test has exclusive access.
        unsafe {
            Pin::new_unchecked(&mut a).register(&mut set, Waker::noop().clone());
        }
        assert!(a.is_registered());
        assert!(!set.is_empty());
    }

    #[test]
    fn update_waker_after_register() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            Pin::new_unchecked(&mut a).register(&mut set, Waker::noop().clone());
            Pin::new_unchecked(&mut a).update_waker(Waker::noop().clone());
        }

        assert!(a.is_registered());
        let waker = set.notify_one();
        assert!(waker.is_some());
        assert!(set.is_empty());
    }

    #[test]
    fn register_with_data_stores_user_data() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            Pin::new_unchecked(&mut a).register_with_data(&mut set, Waker::noop().clone(), 42);
        }
        assert!(a.is_registered());
        assert_eq!(a.user_data(), 42);
    }

    #[test]
    fn unregister_transitions_to_idle() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            Pin::new_unchecked(&mut a).register(&mut set, Waker::noop().clone());
            Pin::new_unchecked(&mut a).unregister(&mut set);
        }

        assert!(!a.is_registered());
        assert!(set.is_empty());
    }

    #[test]
    fn unregister_when_idle_is_noop() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            Pin::new_unchecked(&mut a).unregister(&mut set);
        }
        assert!(!a.is_registered());
    }

    #[test]
    fn take_notification_returns_false_when_idle() {
        let mut a = Awaiter::new();

        // SAFETY: Test has exclusive access.
        let notified = unsafe { Pin::new_unchecked(&mut a).take_notification() };
        assert!(!notified);
    }

    #[test]
    fn notify_transitions_to_notified() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            Pin::new_unchecked(&mut a).register(&mut set, Waker::noop().clone());
        }

        let node = set.notify_one();
        assert!(node.is_some());

        // SAFETY: Test has exclusive access.
        assert!(unsafe { Pin::new_unchecked(&a).is_notified() });
    }

    #[test]
    fn take_notification_transitions_notified_to_idle() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            Pin::new_unchecked(&mut a).register(&mut set, Waker::noop().clone());
        }

        drop(set.notify_one());

        // SAFETY: Test has exclusive access.
        let notified = unsafe { Pin::new_unchecked(&mut a).take_notification() };
        assert!(notified);
        assert!(!a.is_registered());
    }

    #[test]
    fn is_notified_does_not_change_state() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            Pin::new_unchecked(&mut a).register(&mut set, Waker::noop().clone());
        }
        drop(set.notify_one());

        // SAFETY: Test has exclusive access.
        assert!(unsafe { Pin::new_unchecked(&a).is_notified() });
        // Still notified — is_notified does not consume.
        // SAFETY: Test has exclusive access.
        assert!(unsafe { Pin::new_unchecked(&a).is_notified() });
    }

    #[test]
    fn take_waker_without_notify() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            Pin::new_unchecked(&mut a).register(&mut set, Waker::noop().clone());
        }

        let waker = a.take_waker();
        assert!(waker.is_some());
        // Not notified — take_waker does not set notified.
        // SAFETY: Test has exclusive access.
        assert!(!unsafe { Pin::new_unchecked(&a).is_notified() });
    }

    #[test]
    fn full_lifecycle_register_notify_take() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            Pin::new_unchecked(&mut a).register(&mut set, Waker::noop().clone());
        }
        assert!(a.is_registered());

        drop(set.notify_one());

        // SAFETY: Test has exclusive access.
        assert!(unsafe { Pin::new_unchecked(&mut a).take_notification() });
        assert!(!a.is_registered());
    }

    #[test]
    fn full_lifecycle_register_unregister() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            Pin::new_unchecked(&mut a).register(&mut set, Waker::noop().clone());
        }
        assert!(a.is_registered());

        // SAFETY: Test has exclusive access.
        unsafe {
            Pin::new_unchecked(&mut a).unregister(&mut set);
        }
        assert!(!a.is_registered());
        assert!(set.is_empty());
    }
}
