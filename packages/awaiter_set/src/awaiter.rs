use std::cell::UnsafeCell;
use std::marker::PhantomPinned;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::task::Waker;
use std::{fmt, ptr};

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
pub(crate) enum State {
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

/// Represents a single waiting future in an [`AwaiterSet`][crate::AwaiterSet].
///
/// Embed an `Awaiter` in each async future that may need to wait for
/// a synchronization primitive. The primitive registers the awaiter
/// via [`AwaiterSet::register()`][crate::AwaiterSet::register] and later wakes it via
/// [`AwaiterSet::notify_one()`][crate::AwaiterSet::notify_one].
///
/// # Lifecycle
///
/// An awaiter goes through three states:
///
/// 1. **Idle** — just created or after notification was consumed.
/// 2. **Waiting** — registered in a set, holding a waker.
/// 3. **Notified** — removed from the set by the primitive; the
///    owning future should complete with `Ready` on its next poll.
///
/// # Safety model
///
/// The `take_notification` and `is_notified` methods are `unsafe`
/// because the awaiter's state is shared with its [`AwaiterSet`][crate::AwaiterSet].
/// The caller must hold the same lock that protects the set (or
/// confine access to a single thread).
pub struct Awaiter {
    pub(crate) state: UnsafeCell<State>,
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

    /// Returns `true` if the awaiter is currently registered in a
    /// set or has been notified but not yet consumed.
    ///
    /// A future's [`Drop`] handler uses this to decide whether
    /// cleanup (unregistering or forwarding a resource) is needed.
    ///
    /// # Safety
    ///
    /// The awaiter and its set must be protected by the same lock
    /// (or confined to a single thread).
    #[must_use]
    pub unsafe fn is_registered(&self) -> bool {
        // SAFETY: Caller guarantees serialized access.
        !matches!(unsafe { &*self.state.get() }, State::Idle)
    }

    /// Consumes a pending notification, returning `true` if one
    /// was present.
    ///
    /// If the primitive has notified this awaiter (via
    /// [`AwaiterSet::notify_one()`][crate::AwaiterSet::notify_one]), this method returns `true`
    /// and resets the awaiter to the Idle state. The future should
    /// then complete with `Poll::Ready`.
    ///
    /// This is typically the first check in a future's `poll()`.
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

    /// Returns `true` if this awaiter has been notified.
    ///
    /// Unlike [`take_notification()`][Self::take_notification], this
    /// does not consume the notification. Used in a future's
    /// [`Drop`] handler to decide whether the primitive granted a
    /// resource (lock, permit, signal) that must be forwarded to
    /// another awaiter instead of being silently discarded.
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
    ///
    /// # Safety
    ///
    /// The awaiter and its set must be protected by the same lock
    /// (or confined to a single thread).
    #[must_use]
    pub unsafe fn user_data(&self) -> usize {
        // SAFETY: &self is sufficient for reading.
        match unsafe { &*self.state.get() } {
            State::Idle => 0,
            State::Waiting { user_data, .. } | State::Notified { user_data } => *user_data,
        }
    }

    // Crate-internal state transition methods for AwaiterSet.

    /// Transitions from Idle to Waiting with the given waker, data,
    /// and linked pointers.
    pub(crate) fn begin_waiting(
        &mut self,
        waker: Waker,
        data: usize,
        next: *mut Self,
        prev: *mut Self,
    ) {
        // SAFETY: &mut self guarantees exclusive access.
        let state = unsafe { &mut *self.state.get() };
        debug_assert!(
            matches!(state, State::Idle),
            "begin_waiting called on non-idle awaiter"
        );
        *state = State::Waiting {
            waker: Some(waker),
            user_data: data,
            next,
            prev,
        };
    }

    /// Updates the waker and data of a Waiting awaiter without
    /// changing its position in the set.
    pub(crate) fn update_waiting(&mut self, new_waker: Waker, data: usize) {
        // SAFETY: &mut self guarantees exclusive access.
        let state = unsafe { &mut *self.state.get() };
        if let State::Waiting {
            waker, user_data, ..
        } = state
        {
            *waker = Some(new_waker);
            *user_data = data;
        } else {
            debug_assert!(false, "update_waiting called on non-waiting awaiter");
        }
    }

    /// Transitions from Waiting to Idle (cancellation without
    /// notification).
    pub(crate) fn cancel(&mut self) {
        // SAFETY: &mut self guarantees exclusive access.
        let state = unsafe { &mut *self.state.get() };
        debug_assert!(
            matches!(state, State::Waiting { .. }),
            "cancel called on non-waiting awaiter"
        );
        *state = State::Idle;
    }

    /// Returns the linked pointers (next, prev) of a Waiting
    /// awaiter.
    pub(crate) fn neighbors(&self) -> (*mut Self, *mut Self) {
        // SAFETY: Caller serializes access via the external lock.
        match unsafe { &*self.state.get() } {
            State::Waiting { next, prev, .. } => (*next, *prev),
            _ => (ptr::null_mut(), ptr::null_mut()),
        }
    }

    /// Updates the next pointer of a Waiting awaiter.
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
            // SAFETY: Debug output is best-effort; no concurrent
            // mutation during formatting.
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
        assert!(!unsafe { a.is_registered() });
    }

    #[test]
    fn default_awaiter_is_idle() {
        let a = Awaiter::default();
        assert!(!unsafe { a.is_registered() });
    }

    #[test]
    fn register_transitions_to_waiting() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
        }
        assert!(unsafe { a.is_registered() });
        assert!(!unsafe { set.is_empty() });
    }

    #[test]
    fn update_waker_after_register() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
        }

        assert!(unsafe { a.is_registered() });
        let waker = unsafe { set.notify_one() };
        assert!(waker.is_some());
        assert!(unsafe { set.is_empty() });
    }

    #[test]
    fn register_with_data_stores_user_data() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            set.register_with_data(Pin::new_unchecked(&mut a), Waker::noop().clone(), 42);
        }
        assert!(unsafe { a.is_registered() });
        assert_eq!(unsafe { a.user_data() }, 42);
    }

    #[test]
    fn unregister_transitions_to_idle() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
            set.unregister(Pin::new_unchecked(&mut a));
        }

        assert!(!unsafe { a.is_registered() });
        assert!(unsafe { set.is_empty() });
    }

    #[test]
    fn unregister_when_idle_is_noop() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            set.unregister(Pin::new_unchecked(&mut a));
        }
        assert!(!unsafe { a.is_registered() });
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
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
        }

        let node = unsafe { set.notify_one() };
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
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
        }

        drop(unsafe { set.notify_one() });

        // SAFETY: Test has exclusive access.
        let notified = unsafe { Pin::new_unchecked(&mut a).take_notification() };
        assert!(notified);
        assert!(!unsafe { a.is_registered() });
    }

    #[test]
    fn is_notified_does_not_change_state() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
        }
        drop(unsafe { set.notify_one() });

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
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
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
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
        }
        assert!(unsafe { a.is_registered() });

        drop(unsafe { set.notify_one() });

        // SAFETY: Test has exclusive access.
        assert!(unsafe { Pin::new_unchecked(&mut a).take_notification() });
        assert!(!unsafe { a.is_registered() });
    }

    #[test]
    fn full_lifecycle_register_unregister() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
        }
        assert!(unsafe { a.is_registered() });

        // SAFETY: Test has exclusive access.
        unsafe {
            set.unregister(Pin::new_unchecked(&mut a));
        }
        assert!(!unsafe { a.is_registered() });
        assert!(unsafe { set.is_empty() });
    }

    #[test]
    fn unregister_on_notified_is_noop() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
        }

        // Notify (removes from set, transitions to Notified).
        drop(unsafe { set.notify_one() });
        assert!(unsafe { Pin::new_unchecked(&a).is_notified() });

        // Unregister on a Notified awaiter should be a no-op.
        // SAFETY: Test has exclusive access.
        unsafe {
            set.unregister(Pin::new_unchecked(&mut a));
        }
        // Still notified — unregister did not change state.
        assert!(unsafe { Pin::new_unchecked(&a).is_notified() });
    }

    #[test]
    fn re_registration_after_notification() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // First cycle: register → notify → take_notification.
        // SAFETY: Test has exclusive access.
        unsafe {
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
        }
        drop(unsafe { set.notify_one() });
        assert!(unsafe { Pin::new_unchecked(&mut a).take_notification() });
        assert!(!unsafe { a.is_registered() });

        // Second cycle: register again on the same awaiter.
        // SAFETY: Test has exclusive access.
        unsafe {
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
        }
        assert!(unsafe { a.is_registered() });
        assert!(!unsafe { set.is_empty() });

        // Clean up.
        drop(unsafe { set.notify_one() });
    }

    #[test]
    fn take_notification_returns_false_for_waiting() {
        let mut a = Awaiter::new();
        let mut set = AwaiterSet::new();

        // SAFETY: Test has exclusive access.
        unsafe {
            set.register(Pin::new_unchecked(&mut a), Waker::noop().clone());
        }

        // Registered but not notified — take_notification returns false.
        // SAFETY: Test has exclusive access.
        assert!(!unsafe { Pin::new_unchecked(&mut a).take_notification() });
        // Still registered.
        assert!(unsafe { a.is_registered() });
    }
}
