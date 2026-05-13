use std::any::type_name;
use std::cell::UnsafeCell;
use std::fmt;
use std::future::Future;
use std::marker::{PhantomData, PhantomPinned};
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::Rc;
use std::task::{self, Poll, Waker};

use awaiter_set::{Awaiter, AwaiterSet};

/// Single-threaded async auto-reset event.
///
/// Each [`set()`][Self::set] call releases at most one awaiter.
///
/// This is the `!Send` counterpart of [`AutoResetEvent`][crate::AutoResetEvent],
/// more efficient on single-threaded executors.
///
/// # Signal rules
///
/// * If one or more waiters are registered, `set()` releases exactly
///   one waiter and the event stays unset.
/// * If no one is waiting, `set()` stores the signal so that the next
///   [`wait()`][Self::wait] completes immediately (consuming the
///   signal).
/// * Multiple `set()` calls while no one is waiting are coalesced
///   into a single stored signal — only one future waiter is
///   released, not one per `set()` call.
///
/// # Fairness
///
/// The order in which waiters are released is unspecified.
///
/// # Storage
///
/// Use [`boxed()`][Self::boxed] for heap-allocated state (simple,
/// `Clone`-able handles) or [`embedded()`][Self::embedded] to borrow
/// caller-provided storage and avoid the allocation. See the
/// [crate-level documentation](crate) for guidance on when to use
/// each.
///
/// The event is a lightweight cloneable handle. All clones derived
/// from the same origin share the same underlying state.
///
/// # Reentrancy
///
/// A [`Waker`] invoked by this event may re-enter the same event.
/// The following operations are sound when performed from inside a
/// `Waker::wake` callback fired by this event:
///
/// * [`set()`][Self::set] and [`try_wait()`][Self::try_wait]
/// * Creating and polling a fresh [`wait()`][Self::wait] future
/// * Dropping another in-flight [`Future`][std::future::Future] from
///   this event, including one that is still pending
///
/// # Examples
///
/// ```
/// use events::LocalAutoResetEvent;
///
/// #[tokio::main]
/// async fn main() {
///     let local = tokio::task::LocalSet::new();
///     local
///         .run_until(async {
///             let event = LocalAutoResetEvent::boxed();
///             let setter = event.clone();
///
///             // Producer task signals.
///             tokio::task::spawn_local(async move {
///                 setter.set();
///             });
///
///             // Consumer task waits.
///             event.wait().await;
///
///             // Signal was consumed.
///             assert!(!event.try_wait());
///         })
///         .await;
/// }
/// ```
#[derive(Clone)]
pub struct LocalAutoResetEvent {
    inner: Rc<Inner>,
}

// The signal flag and awaiter set are mutually exclusive: if the event is set
// there are no waiters, and if there are waiters the event is not set. This
// enum encodes that invariant at the type level.
enum InnerState {
    /// Not signaled. The awaiter set may be empty or non-empty.
    Unset(AwaiterSet),
    /// Signal stored (will be consumed by the next wait or `try_wait`).
    Set,
}

struct Inner {
    state: UnsafeCell<InnerState>,
    _not_send: PhantomData<*const ()>,
}

// Marker trait impls have no executable code.
impl UnwindSafe for Inner {}
impl RefUnwindSafe for Inner {}

impl Inner {
    fn set(&self) {
        // Capture the waker while borrowing the state, then wake
        // after the borrow ends to avoid aliased mutable access if
        // the waker is reentrant.
        let state_ptr = self.state.get();
        let waker = {
            // SAFETY: Validity — `state_ptr` came from `self.state.get()`, an `UnsafeCell`
            // field of `self` that outlives this borrow. Aliasing — `Inner: !Send`
            // excludes other threads, and the borrow is scoped to this block, ending
            // before the captured waker is invoked; a reentrant `set()` therefore
            // cannot observe an aliasing `&mut`.
            let state = unsafe { &mut *state_ptr };

            match state {
                InnerState::Set => None,
                InnerState::Unset(waiters) => {
                    if let Some(w) = waiters.notify_one() {
                        Some(w)
                    } else {
                        // No waiters — store the signal.
                        *state = InnerState::Set;
                        None
                    }
                }
            }
        };

        if let Some(w) = waker {
            w.wake();
        }
    }

    fn try_wait(&self) -> bool {
        // SAFETY: Validity — `self.state` is an `UnsafeCell` field of `self` that
        // outlives this borrow. Aliasing — `Inner: !Send` excludes other threads, and
        // this function runs no user code while the borrow is live, so no nested or
        // reentrant access can construct an aliasing reference.
        let state = unsafe { &mut *self.state.get() };
        if matches!(state, InnerState::Set) {
            *state = InnerState::Unset(AwaiterSet::new());
            true
        } else {
            false
        }
    }

    /// # Safety
    ///
    /// * The `awaiter` must belong to a future created from the same event.
    unsafe fn poll_wait(&self, mut awaiter: Pin<&mut Awaiter>, waker: Waker) -> Poll<()> {
        if awaiter.as_ref().take_notification() {
            return Poll::Ready(());
        }

        // SAFETY: Validity — `self.state` is an `UnsafeCell` field of `self` that
        // outlives this borrow. Aliasing — `Inner: !Send` excludes other threads; the
        // borrow is held only while invoking internal `&mut AwaiterSet` methods (no
        // user code runs), so no nested or reentrant access can construct an aliasing
        // reference.
        let state = unsafe { &mut *self.state.get() };

        match state {
            InnerState::Set => {
                debug_assert!(
                    !awaiter.is_registered(),
                    "Set state is exclusive with registered waiters"
                );
                *state = InnerState::Unset(AwaiterSet::new());
                Poll::Ready(())
            }
            InnerState::Unset(waiters) => {
                // Register or update the waker.
                // SAFETY: Single-threaded, awaiter is pinned and
                // lives as long as the future.
                unsafe {
                    waiters.register(awaiter.as_mut(), waker);
                }
                Poll::Pending
            }
        }
    }

    /// # Safety
    ///
    /// Same requirements as [`poll_wait`][Self::poll_wait].
    unsafe fn drop_wait(&self, mut awaiter: Pin<&mut Awaiter>) {
        if !awaiter.is_registered() {
            return;
        }

        if awaiter.as_ref().is_notified() {
            // The awaiter caught a notification but the future is
            // being dropped before it could observe it. Hand the
            // notification off to another waiter (if any) or restore
            // the signal so it is not lost.
            //
            // The borrow on the awaiter set is dropped before the
            // returned waker is invoked, so a reentrant call from
            // the woken future observes the canonical state.
            // SAFETY: Validity — `self.state` is an `UnsafeCell` field of `self` that
            // outlives this borrow. Aliasing — `Inner: !Send` excludes other threads,
            // and the borrow is scoped to the `let waker = match state { ... }` chain,
            // ending before `wake()` is invoked; a reentrant call therefore cannot
            // observe an aliasing `&mut`.
            let state = unsafe { &mut *self.state.get() };
            let waker = match state {
                InnerState::Unset(waiters) => match waiters.notify_one() {
                    Some(w) => Some(w),
                    None => {
                        // No more waiters — restore the signal so it
                        // is not lost.
                        *state = InnerState::Set;
                        None
                    }
                },
                InnerState::Set => None,
            };
            if let Some(w) = waker {
                w.wake();
            }
        } else {
            // Not notified — just remove from the set.
            // SAFETY: Validity — `self.state` is an `UnsafeCell` field of `self` that
            // outlives this borrow. Aliasing — `Inner: !Send` excludes other threads,
            // and the borrow is held only while invoking `AwaiterSet::unregister`,
            // which runs no user code.
            let state = unsafe { &mut *self.state.get() };
            match state {
                InnerState::Unset(waiters) => {
                    // SAFETY: Single-threaded, awaiter is registered in
                    // this set.
                    unsafe {
                        waiters.unregister(awaiter.as_mut());
                    }
                }
                InnerState::Set => {
                    // Not notified + registered ⟹ node is in a
                    // awaiter set ⟹ state must be Unset.
                    debug_assert!(false, "registered non-notified node requires Unset state");
                }
            }
        }
    }
}

impl LocalAutoResetEvent {
    /// Creates a new event in the unset state.
    ///
    /// The state is heap-allocated. Clone the handle to share the same
    /// event. For caller-provided storage, see
    /// [`embedded()`][Self::embedded].
    ///
    /// # Examples
    ///
    /// ```
    /// use events::LocalAutoResetEvent;
    ///
    /// let event = LocalAutoResetEvent::boxed();
    /// assert!(!event.try_wait());
    /// ```
    #[must_use]
    pub fn boxed() -> Self {
        Self {
            inner: Rc::new(Inner {
                state: UnsafeCell::new(InnerState::Unset(AwaiterSet::new())),
                _not_send: PhantomData,
            }),
        }
    }

    /// Creates a handle from an [`EmbeddedLocalAutoResetEvent`] container,
    /// avoiding heap allocation.
    ///
    /// Calling this multiple times on the same container is safe and
    /// returns handles that all operate on the same shared state, just
    /// like copying or cloning a [`EmbeddedLocalAutoResetEventRef`].
    ///
    /// # Safety
    ///
    /// The caller must ensure that the [`EmbeddedLocalAutoResetEvent`]
    /// outlives all returned handles and any
    /// [`EmbeddedLocalAutoResetWaitFuture`]s created from them.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::pin::pin;
    ///
    /// use events::{EmbeddedLocalAutoResetEvent, LocalAutoResetEvent};
    ///
    /// # futures::executor::block_on(async {
    /// let container = pin!(EmbeddedLocalAutoResetEvent::new());
    ///
    /// // SAFETY: The container outlives the handle and all wait futures.
    /// let event = unsafe { LocalAutoResetEvent::embedded(container.as_ref()) };
    /// let setter = event;
    ///
    /// setter.set();
    /// event.wait().await;
    /// # });
    /// ```
    #[must_use]
    pub unsafe fn embedded(
        place: Pin<&EmbeddedLocalAutoResetEvent>,
    ) -> EmbeddedLocalAutoResetEventRef {
        let inner = NonNull::from(&place.get_ref().inner);
        EmbeddedLocalAutoResetEventRef { inner }
    }

    /// Signals the event, releasing at most one waiter.
    ///
    /// If one or more waiters are registered, a single waiter is
    /// released and the event remains unset. If no one is waiting,
    /// the event transitions to the set state.
    // Trivial forwarder.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub fn set(&self) {
        self.inner.set();
    }

    /// Attempts to consume the signal without blocking.
    ///
    /// Returns `true` if the event was set, transitioning it back to the
    /// unset state. Returns `false` if the event was not set.
    #[must_use]
    // Trivial forwarder.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub fn try_wait(&self) -> bool {
        self.inner.try_wait()
    }

    /// Returns a future that completes when the event is signaled.
    #[must_use]
    pub fn wait(&self) -> LocalAutoResetWaitFuture {
        LocalAutoResetWaitFuture {
            inner: Rc::clone(&self.inner),
            awaiter: Awaiter::new(),
        }
    }
}

/// Future returned by [`LocalAutoResetEvent::wait()`].
pub struct LocalAutoResetWaitFuture {
    inner: Rc<Inner>,
    awaiter: Awaiter,
}

// Marker trait impls have no executable code.
impl UnwindSafe for LocalAutoResetWaitFuture {}
impl RefUnwindSafe for LocalAutoResetWaitFuture {}

impl Future for LocalAutoResetWaitFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<()> {
        let waker = cx.waker().clone();

        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };
        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut this.awaiter) };
        // SAFETY: The awaiter belongs to this event's awaiter set.
        unsafe { this.inner.poll_wait(awaiter, waker) }
    }
}

impl Drop for LocalAutoResetWaitFuture {
    fn drop(&mut self) {
        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut self.awaiter) };
        // SAFETY: The awaiter belongs to this event's awaiter set.
        unsafe { self.inner.drop_wait(awaiter) }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for LocalAutoResetEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for LocalAutoResetWaitFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            // SAFETY: Debug output is best-effort; no concurrent
            // mutation during formatting.
            .finish_non_exhaustive()
    }
}

/// Embedded-state container for [`LocalAutoResetEvent`].
///
/// Stores the event state inline in a struct, avoiding the heap allocation
/// that [`LocalAutoResetEvent::boxed()`] requires. Create the container
/// with [`new()`][Self::new], pin it, then call
/// [`LocalAutoResetEvent::embedded()`] to obtain a
/// [`EmbeddedLocalAutoResetEventRef`] handle.
///
/// # Examples
///
/// ```
/// use std::pin::pin;
///
/// use events::{EmbeddedLocalAutoResetEvent, LocalAutoResetEvent};
///
/// # futures::executor::block_on(async {
/// let container = pin!(EmbeddedLocalAutoResetEvent::new());
///
/// // SAFETY: The container outlives the handle and all wait futures.
/// let event = unsafe { LocalAutoResetEvent::embedded(container.as_ref()) };
/// let setter = event;
///
/// setter.set();
/// event.wait().await;
/// # });
/// ```
pub struct EmbeddedLocalAutoResetEvent {
    inner: Inner,
    _pinned: PhantomPinned,
}

impl EmbeddedLocalAutoResetEvent {
    /// Creates a new embedded event container in the unset state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Inner {
                state: UnsafeCell::new(InnerState::Unset(AwaiterSet::new())),
                _not_send: PhantomData,
            },
            _pinned: PhantomPinned,
        }
    }
}

impl Default for EmbeddedLocalAutoResetEvent {
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder to new().
    fn default() -> Self {
        Self::new()
    }
}

// Inner already implements UnwindSafe and RefUnwindSafe.
// Marker trait impls have no executable code.
impl UnwindSafe for EmbeddedLocalAutoResetEvent {}
impl RefUnwindSafe for EmbeddedLocalAutoResetEvent {}

/// Handle to an embedded [`LocalAutoResetEvent`].
///
/// Created via [`LocalAutoResetEvent::embedded()`]. The caller is
/// responsible for ensuring the [`EmbeddedLocalAutoResetEvent`] outlives
/// all handles and wait futures.
///
/// The API is identical to [`LocalAutoResetEvent`].
#[derive(Clone, Copy)]
pub struct EmbeddedLocalAutoResetEventRef {
    inner: NonNull<Inner>,
}

// NonNull is !Send and !Sync by default, which is correct for local types.

// Marker trait impls have no executable code.
impl UnwindSafe for EmbeddedLocalAutoResetEventRef {}
impl RefUnwindSafe for EmbeddedLocalAutoResetEventRef {}

impl EmbeddedLocalAutoResetEventRef {
    fn inner(&self) -> &Inner {
        // SAFETY: Validity — the caller of `embedded()` guarantees the container outlives
        // this handle. Aliasing — `Inner`'s API never constructs `&mut Inner` (interior
        // mutability lives behind `UnsafeCell` accessed only via `&self`), so multiple
        // shared references may coexist.
        unsafe { self.inner.as_ref() }
    }

    /// Signals the event, releasing at most one waiter.
    ///
    /// If one or more waiters are registered, a single waiter is
    /// released and the event remains unset. If no one is waiting,
    /// the event transitions to the set state.
    // Trivial forwarder.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub fn set(&self) {
        self.inner().set();
    }

    /// Attempts to consume the signal without blocking.
    ///
    /// Returns `true` if the event was set, transitioning it back to the
    /// unset state. Returns `false` if the event was not set.
    #[must_use]
    // Trivial forwarder.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub fn try_wait(&self) -> bool {
        self.inner().try_wait()
    }

    /// Returns a future that completes when the event is signaled.
    #[must_use]
    pub fn wait(&self) -> EmbeddedLocalAutoResetWaitFuture {
        EmbeddedLocalAutoResetWaitFuture {
            inner: self.inner,
            awaiter: Awaiter::new(),
        }
    }
}

/// Future returned by [`EmbeddedLocalAutoResetEventRef::wait()`].
pub struct EmbeddedLocalAutoResetWaitFuture {
    inner: NonNull<Inner>,
    awaiter: Awaiter,
}

// NonNull makes this !Send and !Sync by default, which is correct for local
// types.

// Marker trait impls have no executable code.
impl UnwindSafe for EmbeddedLocalAutoResetWaitFuture {}
impl RefUnwindSafe for EmbeddedLocalAutoResetWaitFuture {}

impl Future for EmbeddedLocalAutoResetWaitFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<()> {
        let waker = cx.waker().clone();

        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };
        // SAFETY: Validity — the container outlives this future per the `embedded()`
        // contract. Aliasing — `Inner`'s API never constructs `&mut Inner` (interior
        // mutability lives behind `UnsafeCell` accessed only via `&self`), so multiple
        // shared references may coexist.
        let inner = unsafe { this.inner.as_ref() };
        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut this.awaiter) };
        // SAFETY: The awaiter belongs to this event's awaiter set.
        unsafe { inner.poll_wait(awaiter, waker) }
    }
}

impl Drop for EmbeddedLocalAutoResetWaitFuture {
    fn drop(&mut self) {
        // SAFETY: Validity — the container outlives this future per the `embedded()`
        // contract. Aliasing — `Inner`'s API never constructs `&mut Inner` (interior
        // mutability lives behind `UnsafeCell` accessed only via `&self`), so multiple
        // shared references may coexist.
        let inner = unsafe { self.inner.as_ref() };
        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut self.awaiter) };
        // SAFETY: The awaiter belongs to this event's awaiter set.
        unsafe { inner.drop_wait(awaiter) }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for EmbeddedLocalAutoResetEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for EmbeddedLocalAutoResetEventRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for EmbeddedLocalAutoResetWaitFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            // SAFETY: Debug output is best-effort; no concurrent
            // mutation during formatting.
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::iter;
    use std::task::Waker;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(LocalAutoResetEvent: Clone, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(LocalAutoResetEvent: Send, Sync);
    assert_impl_all!(LocalAutoResetWaitFuture: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(LocalAutoResetWaitFuture: Send, Sync, Unpin);

    assert_impl_all!(EmbeddedLocalAutoResetEvent: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(EmbeddedLocalAutoResetEvent: Send, Sync, Unpin);
    assert_impl_all!(EmbeddedLocalAutoResetEventRef: Clone, Copy, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(EmbeddedLocalAutoResetEventRef: Send, Sync);
    assert_impl_all!(EmbeddedLocalAutoResetWaitFuture: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(EmbeddedLocalAutoResetWaitFuture: Send, Sync, Unpin);

    #[test]
    fn starts_unset() {
        let event = LocalAutoResetEvent::boxed();
        assert!(!event.try_wait());
    }

    #[test]
    fn set_then_try_wait() {
        let event = LocalAutoResetEvent::boxed();
        event.set();
        assert!(event.try_wait());
        assert!(!event.try_wait());
    }

    #[test]
    fn clone_shares_state() {
        let a = LocalAutoResetEvent::boxed();
        let b = a.clone();
        a.set();
        assert!(b.try_wait());
    }

    #[test]
    fn wait_completes_when_already_set() {
        futures::executor::block_on(async {
            let event = LocalAutoResetEvent::boxed();
            event.set();
            event.wait().await;
            assert!(!event.try_wait());
        });
    }

    #[test]
    fn wait_completes_after_set() {
        futures::executor::block_on(async {
            let event = LocalAutoResetEvent::boxed();

            // Set before the future is polled.
            let future = event.wait();
            event.set();
            future.await;
        });
    }

    #[test]
    fn drop_future_while_waiting() {
        futures::executor::block_on(async {
            let event = LocalAutoResetEvent::boxed();
            {
                let _f = event.wait();
            }
            event.set();
            event.wait().await;
        });
    }

    #[test]
    fn embedded_set_and_wait() {
        futures::executor::block_on(async {
            let container = Box::pin(EmbeddedLocalAutoResetEvent::new());
            // SAFETY: The container outlives the handle.
            let event = unsafe { LocalAutoResetEvent::embedded(container.as_ref()) };

            event.set();
            event.wait().await;
        });
    }

    #[test]
    fn embedded_clone_shares_state() {
        let container = Box::pin(EmbeddedLocalAutoResetEvent::new());
        // SAFETY: The container outlives the handle.
        let a = unsafe { LocalAutoResetEvent::embedded(container.as_ref()) };
        let b = a;

        a.set();
        assert!(b.try_wait());
    }

    #[test]
    fn embedded_signal_consumed() {
        let container = Box::pin(EmbeddedLocalAutoResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalAutoResetEvent::embedded(container.as_ref()) };

        event.set();
        assert!(event.try_wait());
        assert!(!event.try_wait());
    }

    #[test]
    fn embedded_drop_future_while_waiting() {
        futures::executor::block_on(async {
            let container = Box::pin(EmbeddedLocalAutoResetEvent::new());
            // SAFETY: The container outlives the handle.
            let event = unsafe { LocalAutoResetEvent::embedded(container.as_ref()) };

            {
                let _future = event.wait();
            }
            event.set();
            event.wait().await;
        });
    }

    #[test]
    fn wait_registers_then_completes() {
        let event = LocalAutoResetEvent::boxed();
        let mut future = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        // First poll — not set, registers in awaiter set.
        assert!(future.as_mut().poll(&mut cx).is_pending());

        // set() pops the waiter and marks it notified.
        event.set();

        // Second poll — sees notified flag, returns Ready.
        assert!(future.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn drop_registered_future() {
        let event = LocalAutoResetEvent::boxed();
        let mut future = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(future.as_mut().poll(&mut cx).is_pending());
        drop(future);

        event.set();
        assert!(event.try_wait());
    }

    #[test]
    fn notified_then_dropped_re_sets_event() {
        let event = LocalAutoResetEvent::boxed();
        let mut future = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(future.as_mut().poll(&mut cx).is_pending());
        event.set();
        drop(future);

        // Signal should be preserved.
        assert!(event.try_wait());
    }

    #[test]
    fn dropping_notified_future_while_state_already_set_preserves_signal() {
        // Covers the `InnerState::Set` arm of `drop_wait`. Scenario:
        // 1. Register a future, then `set()` notifies it (transitions
        //    the awaiter to NOTIFIED; state stays `Unset({})`).
        // 2. A second `set()` on the (now-empty) awaiter set
        //    transitions state to `Set`.
        // 3. The still-pinned, still-NOTIFIED future is dropped. Its
        //    `drop_wait` runs with state already in `Set` form.
        let event = LocalAutoResetEvent::boxed();
        let mut future = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(future.as_mut().poll(&mut cx).is_pending());
        // First set: notifies the future (transitions awaiter to
        // NOTIFIED; state remains Unset with an empty awaiter set).
        event.set();
        // Second set: empty awaiter set, so the signal latches —
        // state becomes `InnerState::Set`.
        event.set();

        // Drop the notified future. drop_wait observes
        // `state == InnerState::Set` and must leave the signal in
        // place (the notification belonged to this future and is
        // forfeit; the second set's signal must remain).
        drop(future);

        assert!(event.try_wait());
        assert!(!event.try_wait());
    }

    #[test]
    fn notified_then_dropped_forwards_to_next() {
        let event = LocalAutoResetEvent::boxed();
        let mut future1 = Box::pin(event.wait());
        let mut future2 = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(future1.as_mut().poll(&mut cx).is_pending());
        assert!(future2.as_mut().poll(&mut cx).is_pending());

        event.set();
        drop(future1);

        assert!(future2.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn embedded_wait_registers_then_completes() {
        let container = Box::pin(EmbeddedLocalAutoResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalAutoResetEvent::embedded(container.as_ref()) };

        let mut future = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(future.as_mut().poll(&mut cx).is_pending());
        event.set();
        assert!(future.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn embedded_drop_registered_future() {
        let container = Box::pin(EmbeddedLocalAutoResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalAutoResetEvent::embedded(container.as_ref()) };

        let mut future = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(future.as_mut().poll(&mut cx).is_pending());
        drop(future);

        event.set();
        assert!(event.try_wait());
    }

    #[test]
    fn embedded_notified_then_dropped_re_sets_event() {
        let container = Box::pin(EmbeddedLocalAutoResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalAutoResetEvent::embedded(container.as_ref()) };

        let mut future = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(future.as_mut().poll(&mut cx).is_pending());
        event.set();
        drop(future);

        assert!(event.try_wait());
    }

    #[test]
    fn embedded_notified_then_dropped_forwards_to_next() {
        let container = Box::pin(EmbeddedLocalAutoResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalAutoResetEvent::embedded(container.as_ref()) };

        let mut future1 = Box::pin(event.wait());
        let mut future2 = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(future1.as_mut().poll(&mut cx).is_pending());
        assert!(future2.as_mut().poll(&mut cx).is_pending());

        event.set();
        drop(future1);

        assert!(future2.as_mut().poll(&mut cx).is_ready());
    }
    //
    // These tests use a custom waker that reentrantly accesses the same event
    // when woken. If wake() were called while an &mut AwaiterSet borrow from
    // UnsafeCell is still active, the reentrant access would create aliased
    // mutable references and Miri would flag the UB.

    #[test]
    fn set_with_reentrant_waker_does_not_alias() {
        use testing::ReentrantWakerData;

        let event = LocalAutoResetEvent::boxed();
        let event_clone = event.clone();

        let waker_data = ReentrantWakerData::new(move || {
            // Reentrantly call set(), which accesses the awaiter set.
            event_clone.set();
        });
        // SAFETY: Data outlives waker, single-threaded test.
        let waker = unsafe { waker_data.waker() };
        let mut cx = task::Context::from_waker(&waker);

        let mut future = Box::pin(event.wait());
        assert!(future.as_mut().poll(&mut cx).is_pending());

        // Outer set() pops the waiter, releases the awaiter set borrow,
        // then calls wake(). The reentrant set() safely obtains its own
        // &mut AwaiterSet.
        event.set();

        assert!(waker_data.was_woken());
    }

    #[test]
    fn drop_forwarding_with_reentrant_waker_does_not_alias() {
        use testing::ReentrantWakerData;

        let event = LocalAutoResetEvent::boxed();
        let event_clone = event.clone();

        // future1 uses a noop waker.
        let mut future1 = Box::pin(event.wait());
        let noop_waker = Waker::noop();
        let mut noop_cx = task::Context::from_waker(noop_waker);
        assert!(future1.as_mut().poll(&mut noop_cx).is_pending());

        // future2 uses a reentrant waker.
        let waker_data = ReentrantWakerData::new(move || {
            event_clone.set();
        });
        // SAFETY: Data outlives waker, single-threaded test.
        let waker = unsafe { waker_data.waker() };
        let mut reentrant_cx = task::Context::from_waker(&waker);
        let mut future2 = Box::pin(event.wait());
        assert!(future2.as_mut().poll(&mut reentrant_cx).is_pending());

        // set() notifies future1 (noop waker, harmless).
        event.set();

        // Drop future1 — it was notified, so it forwards to future2,
        // calling the reentrant waker which accesses the awaiter set.
        drop(future1);

        assert!(waker_data.was_woken());
    }

    #[test]
    fn embedded_set_with_reentrant_waker_does_not_alias() {
        use testing::ReentrantWakerData;

        let container = Box::pin(EmbeddedLocalAutoResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalAutoResetEvent::embedded(container.as_ref()) };

        let waker_data = ReentrantWakerData::new(move || {
            event.set();
        });
        // SAFETY: Data outlives waker, single-threaded test.
        let waker = unsafe { waker_data.waker() };
        let mut cx = task::Context::from_waker(&waker);

        let mut future = Box::pin(event.wait());
        assert!(future.as_mut().poll(&mut cx).is_pending());

        event.set();

        assert!(waker_data.was_woken());
    }

    #[test]
    fn embedded_drop_forwarding_with_reentrant_waker_does_not_alias() {
        use testing::ReentrantWakerData;

        let container = Box::pin(EmbeddedLocalAutoResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalAutoResetEvent::embedded(container.as_ref()) };

        // future1 uses a noop waker.
        let mut future1 = Box::pin(event.wait());
        let noop_waker = Waker::noop();
        let mut noop_cx = task::Context::from_waker(noop_waker);
        assert!(future1.as_mut().poll(&mut noop_cx).is_pending());

        // future2 uses a reentrant waker.
        let waker_data = ReentrantWakerData::new(move || {
            event.set();
        });
        // SAFETY: Data outlives waker, single-threaded test.
        let waker = unsafe { waker_data.waker() };
        let mut reentrant_cx = task::Context::from_waker(&waker);
        let mut future2 = Box::pin(event.wait());
        assert!(future2.as_mut().poll(&mut reentrant_cx).is_pending());

        event.set();
        drop(future1);

        assert!(waker_data.was_woken());
    }

    #[test]
    fn embedded_set_wakes_registered_waiter() {
        use testing::ReentrantWakerData;

        let container = Box::pin(EmbeddedLocalAutoResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalAutoResetEvent::embedded(container.as_ref()) };

        let waker_data = ReentrantWakerData::new(|| {});
        // SAFETY: Data outlives waker, single-threaded test.
        let waker = unsafe { waker_data.waker() };
        let mut cx = task::Context::from_waker(&waker);

        let mut future = Box::pin(event.wait());
        assert!(future.as_mut().poll(&mut cx).is_pending());

        event.set();

        assert!(waker_data.was_woken());
    }

    // The tests for forcing "is_set=true while a waiter is registered"
    // were removed because the enum-based InnerState type makes that
    // combination structurally impossible.

    const WAITER_COUNT: usize = 100;

    #[test]
    fn many_sets_release_all_waiters() {
        let event = LocalAutoResetEvent::boxed();
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        let mut futures: Vec<_> = iter::repeat_with(|| Box::pin(event.wait()))
            .take(WAITER_COUNT)
            .collect();

        // Register all waiters.
        for f in &mut futures {
            assert!(f.as_mut().poll(&mut cx).is_pending());
        }

        // Signal once for each waiter.
        for _ in 0..WAITER_COUNT {
            event.set();
        }

        // All waiters should now be ready (order is unspecified).
        for f in &mut futures {
            assert!(f.as_mut().poll(&mut cx).is_ready());
        }

        // No leftover signal.
        assert!(!event.try_wait());
    }

    #[test]
    fn embedded_many_sets_release_all_waiters() {
        let container = Box::pin(EmbeddedLocalAutoResetEvent::new());
        // SAFETY: The container outlives the handle within this test.
        let event = unsafe { LocalAutoResetEvent::embedded(container.as_ref()) };
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        let mut futures: Vec<_> = iter::repeat_with(|| Box::pin(event.wait()))
            .take(WAITER_COUNT)
            .collect();

        for f in &mut futures {
            assert!(f.as_mut().poll(&mut cx).is_pending());
        }

        for _ in 0..WAITER_COUNT {
            event.set();
        }

        for f in &mut futures {
            assert!(f.as_mut().poll(&mut cx).is_ready());
        }

        assert!(!event.try_wait());
    }

    #[test]
    fn many_sets_without_waiters_coalesce() {
        let event = LocalAutoResetEvent::boxed();

        for _ in 0..WAITER_COUNT {
            event.set();
        }

        // Only one signal should be latched.
        assert!(event.try_wait());
        assert!(!event.try_wait());
    }

    #[test]
    fn embedded_default_creates_unset_event() {
        let container = Box::pin(EmbeddedLocalAutoResetEvent::default());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalAutoResetEvent::embedded(container.as_ref()) };
        assert!(!event.try_wait());
    }
}
