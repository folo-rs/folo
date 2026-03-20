use std::cell::{Cell, UnsafeCell};
use std::fmt;
use std::future::Future;
use std::marker::{PhantomData, PhantomPinned};
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::Rc;
use std::task::{self, Poll};

use crate::waiter_list::{WaiterList, WaiterNode};

/// Single-threaded async event that releases exactly one awaiter per
/// [`set()`][Self::set] call.
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
///             assert!(!event.try_acquire());
///         })
///         .await;
/// }
/// ```
#[derive(Clone)]
pub struct LocalAutoResetEvent {
    inner: Rc<Inner>,
}

struct Inner {
    is_set: Cell<bool>,
    waiters: UnsafeCell<WaiterList>,
    _not_send: PhantomData<*const ()>,
}

// Marker trait impls have no executable code.
#[cfg_attr(coverage_nightly, coverage(off))]
impl UnwindSafe for Inner {}
#[cfg_attr(coverage_nightly, coverage(off))]
impl RefUnwindSafe for Inner {}

impl LocalAutoResetEvent {
    /// Creates a new event in the unset state.
    ///
    /// # Examples
    ///
    /// ```
    /// use events::LocalAutoResetEvent;
    ///
    /// let event = LocalAutoResetEvent::boxed();
    /// assert!(!event.try_acquire());
    /// ```
    #[must_use]
    pub fn boxed() -> Self {
        Self {
            inner: Rc::new(Inner {
                is_set: Cell::new(false),
                waiters: UnsafeCell::new(WaiterList::new()),
                _not_send: PhantomData,
            }),
        }
    }

    /// Creates a handle backed by an [`EmbeddedLocalAutoResetEvent`]
    /// container instead of a heap-allocated [`Rc`].
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
    /// // SAFETY: The container outlives the handle.
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
    pub fn set(&self) {
        // Capture the waker while borrowing the waiter list, then wake after
        // the borrow ends to avoid aliased mutable access if the waker is
        // re-entrant.
        let waker = {
            // SAFETY: Single-threaded access guaranteed by !Send.
            let waiters = unsafe { &mut *self.inner.waiters.get() };

            // SAFETY: Single-threaded.
            if let Some(node_ptr) = unsafe { waiters.pop_front() } {
                // SAFETY: Single-threaded, node was just popped.
                unsafe {
                    (*node_ptr).notified = true;
                }

                // SAFETY: Single-threaded.
                unsafe { (*node_ptr).waker.take() }
            } else {
                self.inner.is_set.set(true);
                None
            }
        };

        if let Some(w) = waker {
            w.wake();
        }
    }

    /// Attempts to consume the signal without blocking.
    ///
    /// Returns `true` if the event was set, transitioning it back to the
    /// unset state. Returns `false` if the event was not set.
    #[must_use]
    pub fn try_acquire(&self) -> bool {
        if self.inner.is_set.get() {
            self.inner.is_set.set(false);
            true
        } else {
            false
        }
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
    node: UnsafeCell<WaiterNode>,
    registered: bool,
    _pinned: PhantomPinned,
}

// Marker trait impls have no executable code.
#[cfg_attr(coverage_nightly, coverage(off))]
impl UnwindSafe for LocalAutoResetWaitFuture {}
#[cfg_attr(coverage_nightly, coverage(off))]
impl RefUnwindSafe for LocalAutoResetWaitFuture {}

impl Future for LocalAutoResetWaitFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<()> {
        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };
        let node_ptr = this.node.get();

        // SAFETY: Single-threaded access.
        if unsafe { (*node_ptr).notified } {
            this.registered = false;
            return Poll::Ready(());
        }

        if this.inner.is_set.get() {
            this.inner.is_set.set(false);
            if this.registered {
                // SAFETY: Single-threaded, node is in the list.
                let waiters = unsafe { &mut *this.inner.waiters.get() };
                // SAFETY: Single-threaded, node is registered in this list.
                unsafe {
                    waiters.remove(node_ptr);
                }
                this.registered = false;
            }
            return Poll::Ready(());
        }

        // SAFETY: Single-threaded access.
        unsafe {
            (*node_ptr).waker = Some(cx.waker().clone());
        }

        if !this.registered {
            // SAFETY: Single-threaded, node is pinned and not in any list.
            let waiters = unsafe { &mut *this.inner.waiters.get() };
            // SAFETY: Single-threaded, node is not in any list.
            unsafe {
                waiters.push_back(node_ptr);
            }
            this.registered = true;
        }

        Poll::Pending
    }
}

impl Drop for LocalAutoResetWaitFuture {
    fn drop(&mut self) {
        if !self.registered {
            return;
        }

        let node_ptr = self.node.get();

        // SAFETY: Single-threaded access.
        if unsafe { (*node_ptr).notified } {
            // Cancelled after notification — forward to next waiter.
            // Capture the waker while borrowing the waiter list, then wake
            // after the borrow ends to avoid aliased mutable access if the
            // waker is re-entrant.
            let waker = {
                // SAFETY: Single-threaded.
                let waiters = unsafe { &mut *self.inner.waiters.get() };

                // SAFETY: Single-threaded.
                if let Some(next_node) = unsafe { waiters.pop_front() } {
                    // SAFETY: Single-threaded.
                    unsafe {
                        (*next_node).notified = true;
                    }
                    // SAFETY: Single-threaded.
                    unsafe { (*next_node).waker.take() }
                } else {
                    self.inner.is_set.set(true);
                    None
                }
            };

            if let Some(w) = waker {
                w.wake();
            }
        } else {
            // Not notified — just remove from the list.
            // SAFETY: Single-threaded, node is in the list.
            let waiters = unsafe { &mut *self.inner.waiters.get() };
            // SAFETY: Single-threaded, node is registered in this list.
            unsafe {
                waiters.remove(node_ptr);
            }
        }
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

/// Container for embedding a [`LocalAutoResetEvent`]'s state directly in a
/// struct, avoiding the heap allocation that [`LocalAutoResetEvent::boxed()`]
/// requires.
///
/// Create the container with [`new()`][Self::new], pin it, then call
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
/// // SAFETY: The container outlives the handle.
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
                is_set: Cell::new(false),
                waiters: UnsafeCell::new(WaiterList::new()),
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
#[cfg_attr(coverage_nightly, coverage(off))]
impl UnwindSafe for EmbeddedLocalAutoResetEvent {}
#[cfg_attr(coverage_nightly, coverage(off))]
impl RefUnwindSafe for EmbeddedLocalAutoResetEvent {}

/// Handle to an embedded [`LocalAutoResetEvent`] created via
/// [`LocalAutoResetEvent::embedded()`].
///
/// This handle uses a raw pointer to the embedded state instead of an
/// [`Rc`]. The caller is responsible for ensuring the
/// [`EmbeddedLocalAutoResetEvent`] outlives all handles and wait futures.
///
/// The API is identical to [`LocalAutoResetEvent`].
#[derive(Clone, Copy)]
pub struct RawLocalAutoResetEvent {
    inner: NonNull<Inner>,
}

// NonNull is !Send and !Sync by default, which is correct for local types.

// Marker trait impls have no executable code.
#[cfg_attr(coverage_nightly, coverage(off))]
impl UnwindSafe for RawLocalAutoResetEvent {}
#[cfg_attr(coverage_nightly, coverage(off))]
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
    pub fn set(&self) {
        // Capture the waker while borrowing the waiter list, then wake after
        // the borrow ends to avoid aliased mutable access if the waker is
        // re-entrant.
        let waker = {
            // SAFETY: Single-threaded access guaranteed by !Send.
            let waiters = unsafe { &mut *self.inner().waiters.get() };

            // SAFETY: Single-threaded.
            if let Some(node_ptr) = unsafe { waiters.pop_front() } {
                // SAFETY: Single-threaded, node was just popped from our list.
                unsafe {
                    (*node_ptr).notified = true;
                }

                // SAFETY: Single-threaded.
                unsafe { (*node_ptr).waker.take() }
            } else {
                self.inner().is_set.set(true);
                None
            }
        };

        if let Some(w) = waker {
            w.wake();
        }
    }

    /// Attempts to consume the signal without blocking.
    ///
    /// Returns `true` if the event was set, transitioning it back to the
    /// unset state. Returns `false` if the event was not set.
    #[must_use]
    pub fn try_acquire(&self) -> bool {
        if self.inner().is_set.get() {
            self.inner().is_set.set(false);
            true
        } else {
            false
        }
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
    node: UnsafeCell<WaiterNode>,
    registered: bool,
    _pinned: PhantomPinned,
}

// NonNull and UnsafeCell make this !Send and !Sync by default, which is
// correct for local types.

// Marker trait impls have no executable code.
#[cfg_attr(coverage_nightly, coverage(off))]
impl UnwindSafe for RawLocalAutoResetWaitFuture {}
#[cfg_attr(coverage_nightly, coverage(off))]
impl RefUnwindSafe for RawLocalAutoResetWaitFuture {}

impl Future for RawLocalAutoResetWaitFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<()> {
        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };
        // SAFETY: The container outlives this future per the embedded()
        // contract.
        let inner = unsafe { this.inner.as_ref() };
        let node_ptr = this.node.get();

        // SAFETY: Single-threaded access.
        if unsafe { (*node_ptr).notified } {
            this.registered = false;
            return Poll::Ready(());
        }

        if inner.is_set.get() {
            inner.is_set.set(false);
            if this.registered {
                // SAFETY: Single-threaded, node is in the list.
                let waiters = unsafe { &mut *inner.waiters.get() };
                // SAFETY: Single-threaded, node is registered in this list.
                unsafe {
                    waiters.remove(node_ptr);
                }
                this.registered = false;
            }
            return Poll::Ready(());
        }

        // SAFETY: Single-threaded access.
        unsafe {
            (*node_ptr).waker = Some(cx.waker().clone());
        }

        if !this.registered {
            // SAFETY: Single-threaded, node is pinned and not in any list.
            let waiters = unsafe { &mut *inner.waiters.get() };
            // SAFETY: Single-threaded, node is not in any list.
            unsafe {
                waiters.push_back(node_ptr);
            }
            this.registered = true;
        }

        Poll::Pending
    }
}

impl Drop for RawLocalAutoResetWaitFuture {
    fn drop(&mut self) {
        if !self.registered {
            return;
        }

        let node_ptr = self.node.get();
        // SAFETY: The container outlives this future.
        let inner = unsafe { self.inner.as_ref() };

        // SAFETY: Single-threaded access.
        if unsafe { (*node_ptr).notified } {
            // Cancelled after notification — forward to next waiter.
            // Capture the waker while borrowing the waiter list, then wake
            // after the borrow ends to avoid aliased mutable access if the
            // waker is re-entrant.
            let waker = {
                // SAFETY: Single-threaded.
                let waiters = unsafe { &mut *inner.waiters.get() };

                // SAFETY: Single-threaded.
                if let Some(next_node) = unsafe { waiters.pop_front() } {
                    // SAFETY: Single-threaded.
                    unsafe {
                        (*next_node).notified = true;
                    }
                    // SAFETY: Single-threaded.
                    unsafe { (*next_node).waker.take() }
                } else {
                    inner.is_set.set(true);
                    None
                }
            };

            if let Some(w) = waker {
                w.wake();
            }
        } else {
            // Not notified — just remove from the list.
            // SAFETY: Single-threaded, node is in the list.
            let waiters = unsafe { &mut *inner.waiters.get() };
            // SAFETY: Single-threaded, node is registered in this list.
            unsafe {
                waiters.remove(node_ptr);
            }
        }
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
        assert!(!event.try_acquire());
    }

    #[test]
    fn set_then_try_acquire() {
        let event = LocalAutoResetEvent::boxed();
        event.set();
        assert!(event.try_acquire());
        assert!(!event.try_acquire());
    }

    #[test]
    fn clone_shares_state() {
        let a = LocalAutoResetEvent::boxed();
        let b = a.clone();
        a.set();
        assert!(b.try_acquire());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn wait_completes_when_already_set() {
        let event = LocalAutoResetEvent::boxed();
        event.set();
        event.wait().await;
        assert!(!event.try_acquire());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn wait_completes_after_set() {
        let event = LocalAutoResetEvent::boxed();

        // Set before the future is polled.
        let future = event.wait();
        event.set();
        future.await;
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn drop_future_while_waiting() {
        let event = LocalAutoResetEvent::boxed();
        {
            let _f = event.wait();
        }
        event.set();
        event.wait().await;
    }

    // --- embedded variant tests ---

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn embedded_set_and_wait() {
        let container = Box::pin(EmbeddedLocalAutoResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalAutoResetEvent::embedded(container.as_ref()) };

        event.set();
        event.wait().await;
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn embedded_clone_shares_state() {
        let container = Box::pin(EmbeddedLocalAutoResetEvent::new());
        // SAFETY: The container outlives the handle.
        let a = unsafe { LocalAutoResetEvent::embedded(container.as_ref()) };
        let b = a;

        a.set();
        assert!(b.try_acquire());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn embedded_signal_consumed() {
        let container = Box::pin(EmbeddedLocalAutoResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalAutoResetEvent::embedded(container.as_ref()) };

        event.set();
        assert!(event.try_acquire());
        assert!(!event.try_acquire());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn embedded_drop_future_while_waiting() {
        let container = Box::pin(EmbeddedLocalAutoResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalAutoResetEvent::embedded(container.as_ref()) };

        {
            let _future = event.wait();
        }
        event.set();
        event.wait().await;
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
        assert!(event.try_acquire());
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
        assert!(event.try_acquire());
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
        assert!(event.try_acquire());
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

        assert!(event.try_acquire());
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
        let waker = waker_data.waker();
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
        let waker = waker_data.waker();
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
        let waker = waker_data.waker();
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
        let waker = waker_data.waker();
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
        let waker = waker_data.waker();
        let mut cx = task::Context::from_waker(&waker);

        let mut future = Box::pin(event.wait());
        assert!(future.as_mut().poll(&mut cx).is_pending());

        event.set();

        assert!(waker_data.was_woken());
    }
}
