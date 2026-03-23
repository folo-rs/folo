use std::cell::UnsafeCell;
use std::fmt;
use std::future::Future;
use std::marker::{PhantomData, PhantomPinned};
use std::mem;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::Rc;
use std::task::{self, Poll, Waker};

use crate::waiter_list::{WaiterList, WaiterNode};

/// Single-threaded async auto-reset event.
///
/// Releases exactly one awaiter per [`set()`][Self::set] call.
///
/// This is the `!Send` counterpart of [`AutoResetEvent`][crate::AutoResetEvent].
/// It avoids atomic operations and locking, making it more efficient on
/// single-threaded executors.
///
/// The event is a lightweight cloneable handle. All clones derived from the
/// same [`boxed()`][Self::boxed] call share the same underlying state.
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

// The signal flag and waiter list are mutually exclusive: if the event is set
// there are no waiters, and if there are waiters the event is not set. This
// enum encodes that invariant at the type level.
enum InnerState {
    /// Not signaled. The waiter list may be empty or non-empty.
    Unset(WaiterList),
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
    // Mutating set() to a no-op causes wait futures to hang.
    #[cfg_attr(test, mutants::skip)]
    fn set(&self) {
        // Capture the waker while borrowing the state, then wake
        // after the borrow ends to avoid aliased mutable access if
        // the waker is re-entrant.
        let state_ptr = self.state.get();
        let waker = {
            // SAFETY: Single-threaded access guaranteed by !Send.
            let state = unsafe { &mut *state_ptr };

            match state {
                InnerState::Set => None,
                InnerState::Unset(waiters) => {
                    // SAFETY: Single-threaded.
                    if let Some(node_ptr) = unsafe { waiters.pop_front() } {
                        // SAFETY: Single-threaded, node was just popped.
                        unsafe {
                            (*node_ptr).notified = true;
                        }

                        // SAFETY: Single-threaded.
                        unsafe { (*node_ptr).waker.take() }
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
        // SAFETY: Single-threaded access.
        let state = unsafe { &mut *self.state.get() };
        if matches!(state, InnerState::Set) {
            *state = InnerState::Unset(WaiterList::new());
            true
        } else {
            false
        }
    }

    /// # Safety
    ///
    /// * The `node` must be pinned and must remain at the same memory
    ///   address for the lifetime of the wait future.
    /// * The `node` must belong to a future created from the same event.
    unsafe fn poll_wait(
        &self,
        node: &UnsafeCell<WaiterNode>,
        registered: &mut bool,
        waker: Waker,
    ) -> Poll<()> {
        let node_ptr = node.get();

        // SAFETY: Single-threaded access.
        if unsafe { (*node_ptr).notified } {
            *registered = false;
            return Poll::Ready(());
        }

        // SAFETY: Single-threaded access.
        let state = unsafe { &mut *self.state.get() };

        match state {
            InnerState::Set => {
                debug_assert!(
                    !*registered,
                    "Set state is exclusive with registered waiters"
                );
                *state = InnerState::Unset(WaiterList::new());
                Poll::Ready(())
            }
            InnerState::Unset(waiters) => {
                // SAFETY: Single-threaded access.
                unsafe {
                    (*node_ptr).waker = Some(waker);
                }
                if !*registered {
                    // SAFETY: Single-threaded, node is pinned and
                    // not in any list.
                    unsafe {
                        waiters.push_back(node_ptr);
                    }
                    *registered = true;
                }
                Poll::Pending
            }
        }
    }

    /// # Safety
    ///
    /// Same requirements as [`poll_wait`][Self::poll_wait].
    unsafe fn drop_wait(&self, node: &UnsafeCell<WaiterNode>, registered: bool) {
        if !registered {
            return;
        }

        let node_ptr = node.get();

        // SAFETY: Single-threaded access.
        if unsafe { (*node_ptr).notified } {
            let state_ptr = self.state.get();
            // SAFETY: Single-threaded access.
            let old_state =
                unsafe { mem::replace(&mut *state_ptr, InnerState::Unset(WaiterList::new())) };
            let waker = match old_state {
                InnerState::Unset(mut waiters) => {
                    // SAFETY: Single-threaded.
                    if let Some(next_node) = unsafe { waiters.pop_front() } {
                        // SAFETY: Single-threaded.
                        unsafe {
                            (*next_node).notified = true;
                        }
                        // SAFETY: Single-threaded.
                        let waker = unsafe { (*next_node).waker.take() };
                        // Restore the waiter list.
                        // SAFETY: Single-threaded.
                        unsafe {
                            *state_ptr = InnerState::Unset(waiters);
                        }
                        waker
                    } else {
                        // No more waiters — restore the signal so
                        // it is not lost.
                        // SAFETY: Single-threaded.
                        unsafe {
                            *state_ptr = InnerState::Set;
                        }
                        None
                    }
                }
                InnerState::Set => {
                    // Already set — restore.
                    // SAFETY: Single-threaded.
                    unsafe {
                        *state_ptr = InnerState::Set;
                    }
                    None
                }
            };
            if let Some(w) = waker {
                w.wake();
            }
        } else {
            // Not notified — just remove from the list.
            // SAFETY: Single-threaded access.
            let state = unsafe { &mut *self.state.get() };
            match state {
                InnerState::Unset(waiters) => {
                    // SAFETY: Single-threaded, node is in the list.
                    unsafe {
                        waiters.remove(node_ptr);
                    }
                }
                InnerState::Set => {
                    // Not notified + registered ⟹ node is in a
                    // waiter list ⟹ state must be Unset.
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
                state: UnsafeCell::new(InnerState::Unset(WaiterList::new())),
                _not_send: PhantomData,
            }),
        }
    }

    /// Creates a handle from an [`EmbeddedLocalAutoResetEvent`] container,
    /// avoiding heap allocation.
    ///
    /// Calling this multiple times on the same container is safe and
    /// returns handles that all operate on the same shared state, just
    /// like copying or cloning a [`RawLocalAutoResetEvent`].
    ///
    /// # Safety
    ///
    /// The caller must ensure that the [`EmbeddedLocalAutoResetEvent`]
    /// outlives all returned handles and any
    /// [`RawLocalAutoResetWaitFuture`]s created from them.
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
    pub unsafe fn embedded(place: Pin<&EmbeddedLocalAutoResetEvent>) -> RawLocalAutoResetEvent {
        let inner = NonNull::from(&place.get_ref().inner);
        RawLocalAutoResetEvent { inner }
    }

    /// Signals the event, releasing exactly one waiter.
    ///
    /// If one or more tasks are waiting, a single waiter is released and
    /// the event remains unset. If no task is waiting, the event transitions
    /// to the set state.
    // Mutating set() to a no-op causes wait futures to hang.
    #[cfg_attr(test, mutants::skip)]
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
    ///
    /// # Cancellation safety
    ///
    /// If a notified future is dropped before completion, the notification is
    /// forwarded to the next waiter (or the event is re-set).
    #[must_use]
    pub fn wait(&self) -> LocalAutoResetWaitFuture {
        LocalAutoResetWaitFuture {
            inner: Rc::clone(&self.inner),
            node: UnsafeCell::new(WaiterNode::new()),
            registered: false,
            _pinned: PhantomPinned,
        }
    }
}

/// Future returned by [`LocalAutoResetEvent::wait()`].
pub struct LocalAutoResetWaitFuture {
    inner: Rc<Inner>,

    // Behind UnsafeCell so that raw pointers from the event's waiter list can
    // coexist with the &mut Self we obtain in poll() via get_unchecked_mut().
    // UnsafeCell opts out of the noalias guarantee for its contents.
    node: UnsafeCell<WaiterNode>,

    // Whether this future's node is currently in the event's waiter list.
    // Only accessed through &mut Self in poll()/drop(), never through the list.
    registered: bool,

    _pinned: PhantomPinned,
}

// Marker trait impls have no executable code.
impl UnwindSafe for LocalAutoResetWaitFuture {}
impl RefUnwindSafe for LocalAutoResetWaitFuture {}

impl Future for LocalAutoResetWaitFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<()> {
        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };
        // SAFETY: The node is pinned (PhantomPinned) and belongs to
        // this event's waiter list.
        unsafe {
            this.inner
                .poll_wait(&this.node, &mut this.registered, cx.waker().clone())
        }
    }
}

impl Drop for LocalAutoResetWaitFuture {
    fn drop(&mut self) {
        // SAFETY: The node is pinned (PhantomPinned) and belongs to
        // this event's waiter list.
        unsafe { self.inner.drop_wait(&self.node, self.registered) }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for LocalAutoResetEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalAutoResetEvent")
            .finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for LocalAutoResetWaitFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalAutoResetWaitFuture")
            .field("registered", &self.registered)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Embedded variant
// ---------------------------------------------------------------------------

/// Embedded-state container for [`LocalAutoResetEvent`].
///
/// Stores the event state inline in a struct, avoiding the heap allocation
/// that [`LocalAutoResetEvent::boxed()`] requires. Create the container
/// with [`new()`][Self::new], pin it, then call
/// [`LocalAutoResetEvent::embedded()`] to obtain a
/// [`RawLocalAutoResetEvent`] handle.
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
                state: UnsafeCell::new(InnerState::Unset(WaiterList::new())),
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
pub struct RawLocalAutoResetEvent {
    inner: NonNull<Inner>,
}

// NonNull is !Send and !Sync by default, which is correct for local types.

// Marker trait impls have no executable code.
impl UnwindSafe for RawLocalAutoResetEvent {}
impl RefUnwindSafe for RawLocalAutoResetEvent {}

impl RawLocalAutoResetEvent {
    fn inner(&self) -> &Inner {
        // SAFETY: The caller of `embedded()` guarantees the container
        // outlives this handle.
        unsafe { self.inner.as_ref() }
    }

    /// Signals the event, releasing exactly one waiter.
    ///
    /// If one or more tasks are waiting, a single waiter is released and
    /// the event remains unset. If no task is waiting, the event transitions
    /// to the set state.
    // Mutating set() to a no-op causes wait futures to hang.
    #[cfg_attr(test, mutants::skip)]
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
    ///
    /// # Cancellation safety
    ///
    /// If a notified future is dropped before completion, the notification is
    /// forwarded to the next waiter (or the event is re-set).
    #[must_use]
    pub fn wait(&self) -> RawLocalAutoResetWaitFuture {
        RawLocalAutoResetWaitFuture {
            inner: self.inner,
            node: UnsafeCell::new(WaiterNode::new()),
            registered: false,
            _pinned: PhantomPinned,
        }
    }
}

/// Future returned by [`RawLocalAutoResetEvent::wait()`].
pub struct RawLocalAutoResetWaitFuture {
    inner: NonNull<Inner>,

    // See LocalAutoResetWaitFuture for field documentation.
    node: UnsafeCell<WaiterNode>,
    registered: bool,

    _pinned: PhantomPinned,
}

// NonNull and UnsafeCell make this !Send and !Sync by default, which is
// correct for local types.

// Marker trait impls have no executable code.
impl UnwindSafe for RawLocalAutoResetWaitFuture {}
impl RefUnwindSafe for RawLocalAutoResetWaitFuture {}

impl Future for RawLocalAutoResetWaitFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<()> {
        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };
        // SAFETY: The container outlives this future. Node is pinned via
        // PhantomPinned and belongs to this event's waiter list.
        let inner = unsafe { this.inner.as_ref() };
        // SAFETY: The node is pinned (PhantomPinned) and belongs to
        // this event's waiter list.
        unsafe { inner.poll_wait(&this.node, &mut this.registered, cx.waker().clone()) }
    }
}

impl Drop for RawLocalAutoResetWaitFuture {
    fn drop(&mut self) {
        // SAFETY: The container outlives this future. Node is pinned via
        // PhantomPinned and belongs to this event's waiter list.
        let inner = unsafe { self.inner.as_ref() };
        // SAFETY: The node is pinned (PhantomPinned) and belongs to
        // this event's waiter list.
        unsafe { inner.drop_wait(&self.node, self.registered) }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for EmbeddedLocalAutoResetEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedLocalAutoResetEvent")
            .finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for RawLocalAutoResetEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawLocalAutoResetEvent")
            .finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for RawLocalAutoResetWaitFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawLocalAutoResetWaitFuture")
            .field("registered", &self.registered)
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
    assert_impl_all!(RawLocalAutoResetEvent: Clone, Copy, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(RawLocalAutoResetEvent: Send, Sync);
    assert_impl_all!(RawLocalAutoResetWaitFuture: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(RawLocalAutoResetWaitFuture: Send, Sync, Unpin);

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

    // --- embedded variant tests ---

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

    // --- manual-poll tests (cover register→wake→ready cycle) ---

    #[test]
    fn wait_registers_then_completes() {
        let event = LocalAutoResetEvent::boxed();
        let mut future = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        // First poll — not set, registers in waiter list.
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

    // --- re-entrancy tests (prove wake() is called outside &mut WaiterList borrow) ---
    //
    // These tests use a custom waker that re-entrantly accesses the same event
    // when woken. If wake() were called while an &mut WaiterList borrow from
    // UnsafeCell is still active, the re-entrant access would create aliased
    // mutable references and Miri would flag the UB.

    #[test]
    fn set_with_reentrant_waker_does_not_alias() {
        use crate::test_helpers::ReentrantWakerData;

        let event = LocalAutoResetEvent::boxed();
        let event_clone = event.clone();

        let waker_data = ReentrantWakerData::new(move || {
            // Re-entrantly call set(), which accesses the waiter list.
            event_clone.set();
        });
        // SAFETY: Data outlives waker, single-threaded test.
        let waker = unsafe { waker_data.waker() };
        let mut cx = task::Context::from_waker(&waker);

        let mut future = Box::pin(event.wait());
        assert!(future.as_mut().poll(&mut cx).is_pending());

        // Outer set() pops the waiter, releases the waiter list borrow,
        // then calls wake(). The re-entrant set() safely obtains its own
        // &mut WaiterList.
        event.set();

        assert!(waker_data.was_woken());
    }

    #[test]
    fn drop_forwarding_with_reentrant_waker_does_not_alias() {
        use crate::test_helpers::ReentrantWakerData;

        let event = LocalAutoResetEvent::boxed();
        let event_clone = event.clone();

        // future1 uses a noop waker.
        let mut future1 = Box::pin(event.wait());
        let noop_waker = Waker::noop();
        let mut noop_cx = task::Context::from_waker(noop_waker);
        assert!(future1.as_mut().poll(&mut noop_cx).is_pending());

        // future2 uses a re-entrant waker.
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
        // calling the re-entrant waker which accesses the waiter list.
        drop(future1);

        assert!(waker_data.was_woken());
    }

    #[test]
    fn embedded_set_with_reentrant_waker_does_not_alias() {
        use crate::test_helpers::ReentrantWakerData;

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
        use crate::test_helpers::ReentrantWakerData;

        let container = Box::pin(EmbeddedLocalAutoResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalAutoResetEvent::embedded(container.as_ref()) };

        // future1 uses a noop waker.
        let mut future1 = Box::pin(event.wait());
        let noop_waker = Waker::noop();
        let mut noop_cx = task::Context::from_waker(noop_waker);
        assert!(future1.as_mut().poll(&mut noop_cx).is_pending());

        // future2 uses a re-entrant waker.
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
        use crate::test_helpers::ReentrantWakerData;

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

        // Each set() releases exactly one waiter.
        for f in &mut futures {
            event.set();
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

        for f in &mut futures {
            event.set();
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
}
