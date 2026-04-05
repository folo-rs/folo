use std::cell::{Cell, UnsafeCell};
use std::fmt;
use std::future::Future;
use std::marker::{PhantomData, PhantomPinned};
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::Rc;
use std::task::{self, Poll, Waker};

use awaiter_set::{AwaiterNodeStorage, AwaiterSet};

/// Single-threaded async manual-reset event.
///
/// Once set, releases all current and future awaiters until explicitly reset.
///
/// This is the `!Send` counterpart of [`ManualResetEvent`][crate::ManualResetEvent].
/// It avoids atomic operations and locking, making it more efficient on
/// single-threaded executors.
///
/// The event is a lightweight cloneable handle. All clones derived from the
/// same [`boxed()`][Self::boxed] call share the same underlying state.
///
/// # Examples
///
/// ```
/// use events::LocalManualResetEvent;
///
/// #[tokio::main]
/// async fn main() {
///     let local = tokio::task::LocalSet::new();
///     local
///         .run_until(async {
///             let event = LocalManualResetEvent::boxed();
///             let setter = event.clone();
///
///             // Producer opens the gate from a local task.
///             tokio::task::spawn_local(async move {
///                 setter.set();
///             });
///
///             // Consumer waits for the gate to open.
///             event.wait().await;
///
///             // The gate stays open — it must be explicitly closed.
///             assert!(event.try_wait());
///
///             event.reset();
///             assert!(!event.try_wait());
///         })
///         .await;
/// }
/// ```
#[derive(Clone)]
pub struct LocalManualResetEvent {
    inner: Rc<Inner>,
}

struct Inner {
    /// Whether the event is currently in the signaled state. Unlike
    /// auto-reset events, this is not mutually exclusive with waiters —
    /// waiters stay registered while the event is set and are woken
    /// one-by-one in a loop.
    is_set: Cell<bool>,

    // UnsafeCell because we mutate the set through shared references (Rc).
    // All access is single-threaded, guaranteed by the !Send marker.
    waiters: UnsafeCell<AwaiterSet>,

    // Prevent Send and Sync.
    _not_send: PhantomData<*const ()>,
}

// The Cell and UnsafeCell fields make Inner !RefUnwindSafe by auto-trait
// inference. However, all access is single-threaded and the state machine
// prevents observing inconsistent state during unwind.
// Marker trait impls have no executable code.
impl UnwindSafe for Inner {}
impl RefUnwindSafe for Inner {}

// Test hook that fires after each wake() call in Inner::set(). This allows
// tests to inject re-entrant operations (e.g. calling reset() and re-polling
// a future) between wake iterations, reproducing scenarios that would
// otherwise require specific executor behavior.
#[cfg(test)]
thread_local! {
    static HOOK_SET_AFTER_WAKE: std::cell::RefCell<Option<Box<dyn Fn()>>> =
        const { std::cell::RefCell::new(None) };
}

impl Inner {
    // Mutating set() to a no-op causes wait futures to hang.
    #[cfg_attr(test, mutants::skip)]
    fn set(&self) {
        if self.is_set.get() {
            return;
        }
        self.is_set.set(true);

        // Wake all waiters using the rescan-from-head pattern.
        // We complete the wake call before rescanning because
        // re-entrant wakers may modify the set. By rescanning
        // from the head after each wake, we avoid holding any node
        // pointer across a wake call.
        let waiters_ptr = self.waiters.get();
        loop {
            let waker = {
                // SAFETY: Single-threaded — no concurrent access.
                let waiters = unsafe { &mut *waiters_ptr };
                let mut found = None;
                waiters.for_each(|node| {
                    if found.is_none() {
                        found = node.take_waker();
                    }
                });
                found
            };

            let Some(w) = waker else { break };
            w.wake();

            #[cfg(test)]
            HOOK_SET_AFTER_WAKE.with(|hook| {
                if let Some(f) = hook.borrow().as_ref() {
                    f();
                }
            });

            // If someone called reset() during wake (possibly re-entrantly),
            // stop waking — the gate has been closed.
            if !self.is_set.get() {
                break;
            }
        }
    }

    fn reset(&self) {
        self.is_set.set(false);
    }

    fn try_wait(&self) -> bool {
        self.is_set.get()
    }

    /// # Safety
    ///
    /// * The `slot` must belong to a future created from the same event.
    unsafe fn poll_wait(&self, slot: Pin<&mut AwaiterNodeStorage>, waker: Waker) -> Poll<()> {
        // SAFETY: We do not move the slot.
        let slot = unsafe { slot.get_unchecked_mut() };
        if self.is_set.get() {
            if slot.is_registered() {
                // SAFETY: Single-threaded, slot is registered in this
                // list.
                let waiters = unsafe { &mut *self.waiters.get() };
                // SAFETY: Single-threaded.
                unsafe {
                    slot.unregister(waiters);
                }
            }
            return Poll::Ready(());
        }

        // SAFETY: Single-threaded, slot is pinned and lives as long
        // as the future.
        let waiters = unsafe { &mut *self.waiters.get() };
        // SAFETY: Single-threaded.
        unsafe {
            slot.register(waiters, waker);
        }

        Poll::Pending
    }

    /// # Safety
    ///
    /// Same requirements as [`poll_wait`][Self::poll_wait].
    unsafe fn drop_wait(&self, slot: Pin<&mut AwaiterNodeStorage>) {
        // SAFETY: We do not move the slot.
        let slot = unsafe { slot.get_unchecked_mut() };
        if slot.is_registered() {
            // SAFETY: Single-threaded, slot is registered in this
            // list.
            let waiters = unsafe { &mut *self.waiters.get() };
            // SAFETY: Single-threaded.
            unsafe {
                slot.unregister(waiters);
            }
        }
    }
}

impl LocalManualResetEvent {
    /// Creates a new event in the unset state.
    ///
    /// The state is heap-allocated. Clone the handle to share the same
    /// event. For caller-provided storage, see
    /// [`embedded()`][Self::embedded].
    ///
    /// # Examples
    ///
    /// ```
    /// use events::LocalManualResetEvent;
    ///
    /// let event = LocalManualResetEvent::boxed();
    /// let clone = event.clone();
    ///
    /// clone.set();
    /// assert!(event.try_wait());
    /// ```
    #[must_use]
    pub fn boxed() -> Self {
        Self {
            inner: Rc::new(Inner {
                is_set: Cell::new(false),
                waiters: UnsafeCell::new(AwaiterSet::new()),
                _not_send: PhantomData,
            }),
        }
    }

    /// Creates a handle from an [`EmbeddedLocalManualResetEvent`]
    /// container, avoiding heap allocation.
    ///
    /// Calling this multiple times on the same container is safe and
    /// returns handles that all operate on the same shared state, just
    /// like copying or cloning a [`RawLocalManualResetEvent`].
    ///
    /// # Safety
    ///
    /// The caller must ensure that the [`EmbeddedLocalManualResetEvent`]
    /// outlives all returned handles and any
    /// [`RawLocalManualResetWaitFuture`]s created from them.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::pin::pin;
    ///
    /// use events::{EmbeddedLocalManualResetEvent, LocalManualResetEvent};
    ///
    /// # futures::executor::block_on(async {
    /// let container = pin!(EmbeddedLocalManualResetEvent::new());
    ///
    /// // SAFETY: The container outlives the handle and all wait futures.
    /// let event = unsafe { LocalManualResetEvent::embedded(container.as_ref()) };
    /// let setter = event;
    ///
    /// setter.set();
    /// event.wait().await;
    /// # });
    /// ```
    #[must_use]
    pub unsafe fn embedded(place: Pin<&EmbeddedLocalManualResetEvent>) -> RawLocalManualResetEvent {
        let inner = NonNull::from(&place.get_ref().inner);
        RawLocalManualResetEvent { inner }
    }

    /// Opens the gate, releasing all current awaiters and allowing future
    /// awaiters to pass through immediately.
    ///
    /// If the event is already set, this is a no-op.
    // Trivial forwarder.
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)]
    pub fn set(&self) {
        self.inner.set();
    }

    /// Closes the gate.
    // Trivial forwarder.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub fn reset(&self) {
        self.inner.reset();
    }

    /// Returns `true` if the event is currently set.
    // Trivial forwarder.
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[must_use]
    pub fn try_wait(&self) -> bool {
        self.inner.try_wait()
    }

    /// Returns a future that completes when the event is set.
    #[must_use]
    pub fn wait(&self) -> LocalManualResetWaitFuture {
        LocalManualResetWaitFuture {
            inner: Rc::clone(&self.inner),
            slot: AwaiterNodeStorage::new(),
        }
    }
}

/// Future returned by [`LocalManualResetEvent::wait()`].
pub struct LocalManualResetWaitFuture {
    inner: Rc<Inner>,
    slot: AwaiterNodeStorage,
}

// Marker trait impls have no executable code.
impl UnwindSafe for LocalManualResetWaitFuture {}
impl RefUnwindSafe for LocalManualResetWaitFuture {}

impl Future for LocalManualResetWaitFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<()> {
        let waker = cx.waker().clone();

        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };
        // SAFETY: The slot is pinned inside this future and not moved.
        let slot = unsafe { Pin::new_unchecked(&mut this.slot) };
        // SAFETY: The slot belongs to this event's awaiter set.
        unsafe { this.inner.poll_wait(slot, waker) }
    }
}

impl Drop for LocalManualResetWaitFuture {
    fn drop(&mut self) {
        // SAFETY: The slot is pinned inside this future and not moved.
        let slot = unsafe { Pin::new_unchecked(&mut self.slot) };
        // SAFETY: The slot belongs to this event's awaiter set.
        unsafe {
            self.inner.drop_wait(slot);
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for LocalManualResetEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalManualResetEvent")
            .field("is_set", &self.inner.is_set.get())
            .finish()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for LocalManualResetWaitFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalManualResetWaitFuture")
            .field("registered", &self.slot.is_registered())
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Embedded variant
// ---------------------------------------------------------------------------

/// Embedded-state container for [`LocalManualResetEvent`].
///
/// Stores the event state inline in a struct, avoiding the heap allocation
/// that [`LocalManualResetEvent::boxed()`] requires. Create the container
/// with [`new()`][Self::new], pin it, then call
/// [`LocalManualResetEvent::embedded()`] to obtain a
/// [`RawLocalManualResetEvent`] handle.
///
/// # Examples
///
/// ```
/// use std::pin::pin;
///
/// use events::{EmbeddedLocalManualResetEvent, LocalManualResetEvent};
///
/// # futures::executor::block_on(async {
/// let container = pin!(EmbeddedLocalManualResetEvent::new());
///
/// // SAFETY: The container outlives the handle and all wait futures.
/// let event = unsafe { LocalManualResetEvent::embedded(container.as_ref()) };
/// let waiter = event.clone();
///
/// event.set();
/// waiter.wait().await;
///
/// // The gate stays open — it must be explicitly closed.
/// assert!(event.try_wait());
/// # });
/// ```
pub struct EmbeddedLocalManualResetEvent {
    inner: Inner,
    _pinned: PhantomPinned,
}

impl EmbeddedLocalManualResetEvent {
    /// Creates a new embedded event container in the unset state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Inner {
                is_set: Cell::new(false),
                waiters: UnsafeCell::new(AwaiterSet::new()),
                _not_send: PhantomData,
            },
            _pinned: PhantomPinned,
        }
    }
}

impl Default for EmbeddedLocalManualResetEvent {
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder to new().
    fn default() -> Self {
        Self::new()
    }
}

// Inner already implements UnwindSafe and RefUnwindSafe.
// Marker trait impls have no executable code.
impl UnwindSafe for EmbeddedLocalManualResetEvent {}
impl RefUnwindSafe for EmbeddedLocalManualResetEvent {}

/// Handle to an embedded [`LocalManualResetEvent`].
///
/// Created via [`LocalManualResetEvent::embedded()`]. The caller is
/// responsible for ensuring the [`EmbeddedLocalManualResetEvent`] outlives
/// all handles and wait futures.
///
/// The API is identical to [`LocalManualResetEvent`].
#[derive(Clone, Copy)]
pub struct RawLocalManualResetEvent {
    inner: NonNull<Inner>,
}

// NonNull is !Send and !Sync by default, which is correct for local types.

// Marker trait impls have no executable code.
impl UnwindSafe for RawLocalManualResetEvent {}
impl RefUnwindSafe for RawLocalManualResetEvent {}

impl RawLocalManualResetEvent {
    fn inner(&self) -> &Inner {
        // SAFETY: The caller of `embedded()` guarantees the container
        // outlives this handle.
        unsafe { self.inner.as_ref() }
    }

    /// Opens the gate, releasing all current awaiters.
    ///
    /// If the event is already set, this is a no-op.
    // Trivial forwarder.
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)]
    pub fn set(&self) {
        self.inner().set();
    }

    /// Closes the gate.
    // Trivial forwarder.
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub fn reset(&self) {
        self.inner().reset();
    }

    /// Returns `true` if the event is currently set.
    // Trivial forwarder.
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[must_use]
    pub fn try_wait(&self) -> bool {
        self.inner().try_wait()
    }

    /// Returns a future that completes when the event is set.
    #[must_use]
    pub fn wait(&self) -> RawLocalManualResetWaitFuture {
        RawLocalManualResetWaitFuture {
            inner: self.inner,
            slot: AwaiterNodeStorage::new(),
        }
    }
}

/// Future returned by [`RawLocalManualResetEvent::wait()`].
pub struct RawLocalManualResetWaitFuture {
    inner: NonNull<Inner>,
    slot: AwaiterNodeStorage,
}

// NonNull makes this !Send and !Sync by default, which is correct for local
// types.

// Marker trait impls have no executable code.
impl UnwindSafe for RawLocalManualResetWaitFuture {}
impl RefUnwindSafe for RawLocalManualResetWaitFuture {}

impl Future for RawLocalManualResetWaitFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<()> {
        let waker = cx.waker().clone();

        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };
        // SAFETY: The container outlives this future per the embedded()
        // contract.
        let inner = unsafe { this.inner.as_ref() };
        // SAFETY: The slot is pinned inside this future and not moved.
        let slot = unsafe { Pin::new_unchecked(&mut this.slot) };
        // SAFETY: The slot belongs to this event's awaiter set.
        unsafe { inner.poll_wait(slot, waker) }
    }
}

impl Drop for RawLocalManualResetWaitFuture {
    fn drop(&mut self) {
        // SAFETY: The container outlives this future per the embedded()
        // contract.
        let inner = unsafe { self.inner.as_ref() };
        // SAFETY: The slot is pinned inside this future and not moved.
        let slot = unsafe { Pin::new_unchecked(&mut self.slot) };
        // SAFETY: The slot belongs to this event's awaiter set.
        unsafe {
            inner.drop_wait(slot);
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for EmbeddedLocalManualResetEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedLocalManualResetEvent")
            .finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for RawLocalManualResetEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let is_set = self.try_wait();
        f.debug_struct("RawLocalManualResetEvent")
            .field("is_set", &is_set)
            .finish()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for RawLocalManualResetWaitFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawLocalManualResetWaitFuture")
            .field("registered", &self.slot.is_registered())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::task::Waker;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(LocalManualResetEvent: Clone, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(LocalManualResetEvent: Send, Sync);
    assert_impl_all!(LocalManualResetWaitFuture: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(LocalManualResetWaitFuture: Send, Sync, Unpin);

    assert_impl_all!(EmbeddedLocalManualResetEvent: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(EmbeddedLocalManualResetEvent: Send, Sync, Unpin);
    assert_impl_all!(RawLocalManualResetEvent: Clone, Copy, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(RawLocalManualResetEvent: Send, Sync);
    assert_impl_all!(RawLocalManualResetWaitFuture: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(RawLocalManualResetWaitFuture: Send, Sync, Unpin);

    #[test]
    fn starts_unset() {
        let event = LocalManualResetEvent::boxed();
        assert!(!event.try_wait());
    }

    #[test]
    fn set_and_reset() {
        let event = LocalManualResetEvent::boxed();
        event.set();
        assert!(event.try_wait());
        event.reset();
        assert!(!event.try_wait());
    }

    #[test]
    fn clone_shares_state() {
        let a = LocalManualResetEvent::boxed();
        let b = a.clone();
        a.set();
        assert!(b.try_wait());
    }

    #[test]
    fn wait_completes_when_already_set() {
        futures::executor::block_on(async {
            let event = LocalManualResetEvent::boxed();
            event.set();
            event.wait().await;
        });
    }

    #[test]
    fn wait_completes_after_set() {
        futures::executor::block_on(async {
            let event = LocalManualResetEvent::boxed();

            // Set before the future is polled.
            let future = event.wait();
            event.set();
            future.await;
        });
    }

    #[test]
    fn drop_future_while_waiting() {
        futures::executor::block_on(async {
            let event = LocalManualResetEvent::boxed();
            {
                let _f = event.wait();
            }
            event.set();
            event.wait().await;
        });
    }

    // --- embedded variant tests ---

    #[test]
    fn embedded_set_and_wait() {
        futures::executor::block_on(async {
            let container = Box::pin(EmbeddedLocalManualResetEvent::new());
            // SAFETY: The container outlives the handle.
            let event = unsafe { LocalManualResetEvent::embedded(container.as_ref()) };

            event.set();
            event.wait().await;
        });
    }

    #[test]
    fn embedded_clone_shares_state() {
        futures::executor::block_on(async {
            let container = Box::pin(EmbeddedLocalManualResetEvent::new());
            // SAFETY: The container outlives the handle.
            let a = unsafe { LocalManualResetEvent::embedded(container.as_ref()) };
            let b = a;

            a.set();
            assert!(b.try_wait());
            b.wait().await;
        });
    }

    #[test]
    fn embedded_reset_after_set() {
        let container = Box::pin(EmbeddedLocalManualResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalManualResetEvent::embedded(container.as_ref()) };

        event.set();
        event.reset();
        assert!(!event.try_wait());
    }

    #[test]
    fn embedded_drop_future_while_waiting() {
        futures::executor::block_on(async {
            let container = Box::pin(EmbeddedLocalManualResetEvent::new());
            // SAFETY: The container outlives the handle.
            let event = unsafe { LocalManualResetEvent::embedded(container.as_ref()) };

            {
                let _future = event.wait();
            }
            event.set();
            event.wait().await;
        });
    }

    // --- manual-poll tests (cover register→wake→ready cycle) ---

    #[test]
    fn wait_registers_then_completes() {
        let event = LocalManualResetEvent::boxed();
        let mut future = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        // First poll — not set, registers in awaiter set.
        assert!(future.as_mut().poll(&mut cx).is_pending());

        // Set the event — wakes the registered waiter.
        event.set();

        // Second poll — event is set, returns Ready and unregisters.
        assert!(future.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn drop_registered_future() {
        let event = LocalManualResetEvent::boxed();
        let mut future = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(future.as_mut().poll(&mut cx).is_pending());
        drop(future);

        // Event should still work.
        event.set();
        let mut future2 = Box::pin(event.wait());
        assert!(future2.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn embedded_wait_registers_then_completes() {
        let container = Box::pin(EmbeddedLocalManualResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalManualResetEvent::embedded(container.as_ref()) };

        let mut future = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(future.as_mut().poll(&mut cx).is_pending());
        event.set();
        assert!(future.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn embedded_drop_registered_future() {
        let container = Box::pin(EmbeddedLocalManualResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalManualResetEvent::embedded(container.as_ref()) };

        let mut future = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(future.as_mut().poll(&mut cx).is_pending());
        drop(future);

        event.set();
        let mut future2 = Box::pin(event.wait());
        assert!(future2.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn embedded_multiple_waiters_released() {
        let container = Box::pin(EmbeddedLocalManualResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalManualResetEvent::embedded(container.as_ref()) };

        let mut f1 = Box::pin(event.wait());
        let mut f2 = Box::pin(event.wait());
        let mut f3 = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());
        assert!(f3.as_mut().poll(&mut cx).is_pending());

        event.set();

        assert!(f1.as_mut().poll(&mut cx).is_ready());
        assert!(f2.as_mut().poll(&mut cx).is_ready());
        assert!(f3.as_mut().poll(&mut cx).is_ready());
    }

    // --- re-entrancy tests (prove wake() is called outside awaiter set borrow) ---
    //
    // These tests use a custom waker that re-entrantly accesses the same event
    // when woken. If wake() were called while a &AwaiterSet borrow from
    // UnsafeCell is still active, the re-entrant mutable access would create
    // aliased references and Miri would flag the UB.

    #[test]
    fn set_with_reentrant_waker_does_not_alias() {
        use crate::test_helpers::ReentrantWakerData;

        let event = LocalManualResetEvent::boxed();
        let event_clone = event.clone();

        let waker_data = ReentrantWakerData::new(move || {
            // Re-entrantly call reset() + poll a new wait() future, which
            // accesses the awaiter set to register a new node.
            event_clone.reset();
            let mut new_future = Box::pin(event_clone.wait());
            let noop = Waker::noop();
            let mut cx = task::Context::from_waker(noop);
            assert!(new_future.as_mut().poll(&mut cx).is_pending());
        });
        // SAFETY: Data outlives waker, single-threaded test.
        let waker = unsafe { waker_data.waker() };
        let mut cx = task::Context::from_waker(&waker);

        let mut future = Box::pin(event.wait());
        assert!(future.as_mut().poll(&mut cx).is_pending());

        // set() collects the waker from the set, releases the borrow, then
        // calls wake(). The re-entrant waker resets the event and polls a new
        // future that registers in the awaiter set.
        event.set();

        assert!(waker_data.was_woken());
    }

    #[test]
    fn embedded_set_with_reentrant_waker_does_not_alias() {
        use crate::test_helpers::ReentrantWakerData;

        let container = Box::pin(EmbeddedLocalManualResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalManualResetEvent::embedded(container.as_ref()) };

        let waker_data = ReentrantWakerData::new(move || {
            event.reset();
            let mut new_future = Box::pin(event.wait());
            let noop = Waker::noop();
            let mut cx = task::Context::from_waker(noop);
            assert!(new_future.as_mut().poll(&mut cx).is_pending());
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
    fn try_wait_returns_false_when_unset() {
        let event = LocalManualResetEvent::boxed();
        assert!(!event.try_wait());
    }

    #[test]
    fn try_wait_returns_true_when_set() {
        let event = LocalManualResetEvent::boxed();
        event.set();
        assert!(event.try_wait());
    }

    #[test]
    fn embedded_try_wait_returns_false_when_unset() {
        let container = Box::pin(EmbeddedLocalManualResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalManualResetEvent::embedded(container.as_ref()) };
        assert!(!event.try_wait());
    }

    #[test]
    fn embedded_try_wait_returns_true_when_set() {
        let container = Box::pin(EmbeddedLocalManualResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalManualResetEvent::embedded(container.as_ref()) };
        event.set();
        assert!(event.try_wait());
    }

    #[test]
    fn embedded_set_wakes_registered_waiter() {
        use crate::test_helpers::ReentrantWakerData;

        let container = Box::pin(EmbeddedLocalManualResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalManualResetEvent::embedded(container.as_ref()) };

        let waker_data = ReentrantWakerData::new(|| {});
        // SAFETY: Data outlives waker, single-threaded test.
        let waker = unsafe { waker_data.waker() };
        let mut cx = task::Context::from_waker(&waker);

        let mut future = Box::pin(event.wait());
        assert!(future.as_mut().poll(&mut cx).is_pending());

        event.set();

        assert!(waker_data.was_woken());
    }

    #[test]
    fn drop_unlinks_registered_waiter_from_list() {
        use crate::test_helpers::ReentrantWakerData;

        let event = LocalManualResetEvent::boxed();

        let tracker1 = ReentrantWakerData::new(|| {});
        // SAFETY: Data outlives waker, single-threaded test.
        let waker1 = unsafe { tracker1.waker() };
        let mut cx1 = task::Context::from_waker(&waker1);

        let tracker2 = ReentrantWakerData::new(|| {});
        // SAFETY: Data outlives waker, single-threaded test.
        let waker2 = unsafe { tracker2.waker() };
        let mut cx2 = task::Context::from_waker(&waker2);

        let mut future1 = Box::pin(event.wait());
        assert!(future1.as_mut().poll(&mut cx1).is_pending());

        let mut future2 = Box::pin(event.wait());
        assert!(future2.as_mut().poll(&mut cx2).is_pending());

        // Drop future1 — its node must be removed from the set.
        drop(future1);

        event.set();

        // Only future2 should have been woken.
        assert!(!tracker1.was_woken());
        assert!(tracker2.was_woken());
    }

    #[test]
    fn embedded_drop_unlinks_registered_waiter_from_list() {
        use crate::test_helpers::ReentrantWakerData;

        let container = Box::pin(EmbeddedLocalManualResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalManualResetEvent::embedded(container.as_ref()) };

        let tracker1 = ReentrantWakerData::new(|| {});
        // SAFETY: Data outlives waker, single-threaded test.
        let waker1 = unsafe { tracker1.waker() };
        let mut cx1 = task::Context::from_waker(&waker1);

        let tracker2 = ReentrantWakerData::new(|| {});
        // SAFETY: Data outlives waker, single-threaded test.
        let waker2 = unsafe { tracker2.waker() };
        let mut cx2 = task::Context::from_waker(&waker2);

        let mut future1 = Box::pin(event.wait());
        assert!(future1.as_mut().poll(&mut cx1).is_pending());

        let mut future2 = Box::pin(event.wait());
        assert!(future2.as_mut().poll(&mut cx2).is_pending());

        drop(future1);

        event.set();

        assert!(!tracker1.was_woken());
        assert!(tracker2.was_woken());
    }

    #[test]
    fn set_when_already_set_is_noop() {
        let event = LocalManualResetEvent::boxed();
        event.set();
        assert!(event.try_wait());

        // Second set() should be a no-op (early return).
        event.set();
        assert!(event.try_wait());
    }

    #[test]
    fn embedded_set_when_already_set_is_noop() {
        let container = Box::pin(EmbeddedLocalManualResetEvent::new());

        // SAFETY: The container is pinned and outlives the handle.
        let event = unsafe { LocalManualResetEvent::embedded(container.as_ref()) };

        event.set();
        assert!(event.try_wait());

        event.set();
        assert!(event.try_wait());
    }

    // --- livelock regression test ---

    /// Verifies that `set()` terminates even if `reset()` is called
    /// re-entrantly while the wake loop is in progress.
    ///
    /// Without the `is_set` re-check after each wake, `set()` would loop
    /// forever: it wakes a waiter, the hook resets the event and re-polls
    /// the future (re-storing a waker), and `set()` rescans from head —
    /// finding the fresh waker and repeating indefinitely.
    #[test]
    fn set_terminates_when_reset_called_during_wake_loop() {
        use std::cell::RefCell;
        use std::rc::Rc;

        testing::with_watchdog(|| {
            let event = LocalManualResetEvent::boxed();
            let future = Box::pin(event.wait());

            // First poll: register as a waiter with a noop waker.
            let waker = Waker::noop();
            let mut cx = task::Context::from_waker(waker);
            let future_cell: Rc<RefCell<Pin<Box<LocalManualResetWaitFuture>>>> =
                Rc::new(RefCell::new(future));
            assert!(future_cell.borrow_mut().as_mut().poll(&mut cx).is_pending());

            // Install hook: after each wake(), reset the event and re-poll
            // the future so it re-stores a waker. This simulates a
            // re-entrant waker that resets the event and immediately
            // re-polls.
            let event_for_hook = event.clone();
            let future_for_hook = Rc::clone(&future_cell);

            HOOK_SET_AFTER_WAKE.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    event_for_hook.reset();
                    let w = Waker::noop();
                    let mut cx = task::Context::from_waker(w);
                    let _poll = future_for_hook.borrow_mut().as_mut().poll(&mut cx);
                }));
            });

            event.set();

            // Cleanup.
            HOOK_SET_AFTER_WAKE.with(|hook| {
                *hook.borrow_mut() = None;
            });
        });
    }
}
