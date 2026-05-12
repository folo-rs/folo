use std::any::type_name;
use std::cell::{Cell, UnsafeCell};
use std::fmt;
use std::future::Future;
use std::marker::{PhantomData, PhantomPinned};
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::Rc;
use std::task::{self, Poll, Waker};

use awaiter_set::{Awaiter, AwaiterSet};

/// Single-threaded async manual-reset event.
///
/// Once set, releases all current and future awaiters until explicitly reset.
///
/// A `LocalManualResetEvent` acts as a gate: while set, every call to
/// [`wait()`][Self::wait] completes immediately. Calling
/// [`reset()`][Self::reset] closes the gate so that subsequent
/// awaiters block until the next [`set()`][Self::set].
///
/// This is the `!Send` counterpart of [`ManualResetEvent`][crate::ManualResetEvent].
/// It avoids atomic operations and locking, making it more efficient on
/// single-threaded executors.
///
/// # Fairness
///
/// When `set()` is called, all currently registered waiters are
/// released. The order in which they are woken is unspecified.
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
/// * [`set()`][Self::set], [`reset()`][Self::reset] and
///   [`try_wait()`][Self::try_wait]
/// * Creating and polling a fresh [`wait()`][Self::wait] future
///   (registers a new awaiter)
/// * Dropping another in-flight [`Future`][std::future::Future] from
///   this event, including one that is still pending
///
/// The event always drops any internal borrow on its awaiter set
/// before calling [`Waker::wake()`], so reentrant operations never
/// observe partially mutated state.
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

impl Inner {
    fn set(&self) {
        if self.is_set.get() {
            return;
        }
        self.is_set.set(true);

        // Notify waiters one at a time, releasing the borrow on
        // `self.waiters` before each `wake()` call. A reentrant
        // waker may drop or register another future, which will
        // need to acquire its own `&mut` to the same set; keeping
        // the loop driven by `self.waiters` (rather than a snapshot)
        // ensures unregister/register during reentrancy always
        // targets the live set.
        loop {
            let waker = {
                // SAFETY: Validity — `self.waiters` is an `UnsafeCell` field of `self`
                // that outlives this borrow. Aliasing — `Inner: !Send` excludes other
                // threads, and the borrow is scoped to this block, ending before
                // `wake()` is invoked; a reentrant call therefore cannot observe an
                // aliasing `&mut`.
                let waiters = unsafe { &mut *self.waiters.get() };
                waiters.notify_one()
            };

            match waker {
                Some(w) => w.wake(),
                None => break,
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
    /// * The `awaiter` must belong to a future created from the same event.
    unsafe fn poll_wait(&self, mut awaiter: Pin<&mut Awaiter>, waker: Waker) -> Poll<()> {
        // Check if we were directly notified by set() (it removed us
        // from the set and set our notified flag).
        if awaiter.as_ref().take_notification() {
            return Poll::Ready(());
        }

        if self.is_set.get() {
            return Poll::Ready(());
        }

        // Register or update the waker.
        // SAFETY: Validity — `self.waiters` is an `UnsafeCell` field of `self` that
        // outlives this borrow. Aliasing — `Inner: !Send` excludes other threads, and
        // the borrow is held only while invoking `AwaiterSet::register`, which runs no
        // user code.
        let waiters = unsafe { &mut *self.waiters.get() };
        // SAFETY: Single-threaded.
        unsafe {
            waiters.register(awaiter.as_mut(), waker);
        }

        Poll::Pending
    }

    /// # Safety
    ///
    /// Same requirements as [`poll_wait`][Self::poll_wait].
    unsafe fn drop_wait(&self, mut awaiter: Pin<&mut Awaiter>) {
        if awaiter.is_registered() {
            // SAFETY: Validity — `self.waiters` is an `UnsafeCell` field of `self` that
            // outlives this borrow. Aliasing — `Inner: !Send` excludes other threads,
            // and the borrow is held only while invoking `AwaiterSet::unregister`,
            // which runs no user code.
            let waiters = unsafe { &mut *self.waiters.get() };
            // SAFETY: Single-threaded.
            unsafe {
                waiters.unregister(awaiter.as_mut());
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
    /// like copying or cloning a [`EmbeddedLocalManualResetEventRef`].
    ///
    /// # Safety
    ///
    /// The caller must ensure that the [`EmbeddedLocalManualResetEvent`]
    /// outlives all returned handles and any
    /// [`EmbeddedLocalManualResetWaitFuture`]s created from them.
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
    pub unsafe fn embedded(
        place: Pin<&EmbeddedLocalManualResetEvent>,
    ) -> EmbeddedLocalManualResetEventRef {
        let inner = NonNull::from(&place.get_ref().inner);
        EmbeddedLocalManualResetEventRef { inner }
    }

    /// Opens the gate, releasing all current awaiters and allowing future
    /// awaiters to pass through immediately.
    ///
    /// If the event is already set, this is a no-op.
    // Trivial forwarder.
    #[cfg_attr(coverage_nightly, coverage(off))]
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
            awaiter: Awaiter::new(),
        }
    }
}

/// Future returned by [`LocalManualResetEvent::wait()`].
pub struct LocalManualResetWaitFuture {
    inner: Rc<Inner>,
    awaiter: Awaiter,
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
        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut this.awaiter) };
        // SAFETY: The awaiter belongs to this event's awaiter set.
        unsafe { this.inner.poll_wait(awaiter, waker) }
    }
}

impl Drop for LocalManualResetWaitFuture {
    fn drop(&mut self) {
        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut self.awaiter) };
        // SAFETY: The awaiter belongs to this event's awaiter set.
        unsafe {
            self.inner.drop_wait(awaiter);
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for LocalManualResetEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("is_set", &self.inner.is_set.get())
            .finish()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for LocalManualResetWaitFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            // SAFETY: Debug output is best-effort; no concurrent
            // mutation during formatting.
            .finish_non_exhaustive()
    }
}

/// Embedded-state container for [`LocalManualResetEvent`].
///
/// Stores the event state inline in a struct, avoiding the heap allocation
/// that [`LocalManualResetEvent::boxed()`] requires. Create the container
/// with [`new()`][Self::new], pin it, then call
/// [`LocalManualResetEvent::embedded()`] to obtain a
/// [`EmbeddedLocalManualResetEventRef`] handle.
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
pub struct EmbeddedLocalManualResetEventRef {
    inner: NonNull<Inner>,
}

// NonNull is !Send and !Sync by default, which is correct for local types.

// Marker trait impls have no executable code.
impl UnwindSafe for EmbeddedLocalManualResetEventRef {}
impl RefUnwindSafe for EmbeddedLocalManualResetEventRef {}

impl EmbeddedLocalManualResetEventRef {
    fn inner(&self) -> &Inner {
        // SAFETY: Validity — the caller of `embedded()` guarantees the container outlives
        // this handle. Aliasing — `Inner`'s API never constructs `&mut Inner` (interior
        // mutability lives behind `Cell`/`UnsafeCell` accessed only via `&self`), so
        // multiple shared references may coexist.
        unsafe { self.inner.as_ref() }
    }

    /// Opens the gate, releasing all current awaiters.
    ///
    /// If the event is already set, this is a no-op.
    // Trivial forwarder.
    #[cfg_attr(coverage_nightly, coverage(off))]
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
    pub fn wait(&self) -> EmbeddedLocalManualResetWaitFuture {
        EmbeddedLocalManualResetWaitFuture {
            inner: self.inner,
            awaiter: Awaiter::new(),
        }
    }
}

/// Future returned by [`EmbeddedLocalManualResetEventRef::wait()`].
pub struct EmbeddedLocalManualResetWaitFuture {
    inner: NonNull<Inner>,
    awaiter: Awaiter,
}

// NonNull makes this !Send and !Sync by default, which is correct for local
// types.

// Marker trait impls have no executable code.
impl UnwindSafe for EmbeddedLocalManualResetWaitFuture {}
impl RefUnwindSafe for EmbeddedLocalManualResetWaitFuture {}

impl Future for EmbeddedLocalManualResetWaitFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<()> {
        let waker = cx.waker().clone();

        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };
        // SAFETY: Validity — the container outlives this future per the `embedded()`
        // contract. Aliasing — `Inner`'s API never constructs `&mut Inner` (interior
        // mutability lives behind `Cell`/`UnsafeCell` accessed only via `&self`), so
        // multiple shared references may coexist.
        let inner = unsafe { this.inner.as_ref() };
        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut this.awaiter) };
        // SAFETY: The awaiter belongs to this event's awaiter set.
        unsafe { inner.poll_wait(awaiter, waker) }
    }
}

impl Drop for EmbeddedLocalManualResetWaitFuture {
    fn drop(&mut self) {
        // SAFETY: Validity — the container outlives this future per the `embedded()`
        // contract. Aliasing — `Inner`'s API never constructs `&mut Inner` (interior
        // mutability lives behind `Cell`/`UnsafeCell` accessed only via `&self`), so
        // multiple shared references may coexist.
        let inner = unsafe { self.inner.as_ref() };
        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut self.awaiter) };
        // SAFETY: The awaiter belongs to this event's awaiter set.
        unsafe {
            inner.drop_wait(awaiter);
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for EmbeddedLocalManualResetEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>()).finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for EmbeddedLocalManualResetEventRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let is_set = self.try_wait();
        f.debug_struct(type_name::<Self>())
            .field("is_set", &is_set)
            .finish()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for EmbeddedLocalManualResetWaitFuture {
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
    use std::task::Waker;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(LocalManualResetEvent: Clone, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(LocalManualResetEvent: Send, Sync);
    assert_impl_all!(LocalManualResetWaitFuture: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(LocalManualResetWaitFuture: Send, Sync, Unpin);

    assert_impl_all!(EmbeddedLocalManualResetEvent: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(EmbeddedLocalManualResetEvent: Send, Sync, Unpin);
    assert_impl_all!(EmbeddedLocalManualResetEventRef: Clone, Copy, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(EmbeddedLocalManualResetEventRef: Send, Sync);
    assert_impl_all!(EmbeddedLocalManualResetWaitFuture: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(EmbeddedLocalManualResetWaitFuture: Send, Sync, Unpin);

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
    //
    // These tests use a custom waker that reentrantly accesses the same event
    // when woken. If wake() were called while a &AwaiterSet borrow from
    // UnsafeCell is still active, the reentrant mutable access would create
    // aliased references and Miri would flag the UB.

    #[test]
    fn set_with_reentrant_waker_does_not_alias() {
        use testing::ReentrantWakerData;

        let event = LocalManualResetEvent::boxed();
        let event_clone = event.clone();

        let waker_data = ReentrantWakerData::new(move || {
            // Reentrantly call reset() + poll a new wait() future, which
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
        // calls wake(). The reentrant waker resets the event and polls a new
        // future that registers in the awaiter set.
        event.set();

        assert!(waker_data.was_woken());
    }

    #[test]
    fn embedded_set_with_reentrant_waker_does_not_alias() {
        use testing::ReentrantWakerData;

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
        use testing::ReentrantWakerData;

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
        use testing::ReentrantWakerData;

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
        use testing::ReentrantWakerData;

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

    #[test]
    fn drop_forwarding_with_reentrant_waker_does_not_alias() {
        use testing::ReentrantWakerData;

        let event = LocalManualResetEvent::boxed();
        let event_clone = event.clone();

        // future1 uses a noop waker.
        let mut future1 = Box::pin(event.wait());
        let noop_waker = Waker::noop();
        let mut noop_cx = task::Context::from_waker(noop_waker);
        assert!(future1.as_mut().poll(&mut noop_cx).is_pending());

        // future2 uses a reentrant waker that calls set().
        let waker_data = ReentrantWakerData::new(move || {
            event_clone.set();
        });
        // SAFETY: Data outlives waker, single-threaded test.
        let waker = unsafe { waker_data.waker() };
        let mut reentrant_cx = task::Context::from_waker(&waker);
        let mut future2 = Box::pin(event.wait());
        assert!(future2.as_mut().poll(&mut reentrant_cx).is_pending());

        // set() wakes both futures. future2's reentrant waker
        // calls set() again, accessing the awaiter set.
        event.set();

        // Drop future1 after notification — unlinks from list.
        drop(future1);

        assert!(waker_data.was_woken());
    }

    #[test]
    fn reentrant_reset_does_not_skip_awaiters() {
        use testing::ReentrantWakerData;

        // Register two awaiters A (with reentrant waker) and B (noop).
        // A's waker calls reset(). Both A and B should still be notified
        // because they were registered when set() was called.
        let event = LocalManualResetEvent::boxed();
        let event_for_waker = event.clone();

        let waker_data_a = ReentrantWakerData::new(move || {
            // Reentrantly reset the event.
            event_for_waker.reset();
        });
        // SAFETY: Data outlives waker, single-threaded test.
        let waker_a = unsafe { waker_data_a.waker() };
        let mut cx_a = task::Context::from_waker(&waker_a);

        let noop = Waker::noop();
        let mut cx_b = task::Context::from_waker(noop);

        // Register A and B.
        let mut future_a = Box::pin(event.wait());
        assert!(future_a.as_mut().poll(&mut cx_a).is_pending());

        let mut future_b = Box::pin(event.wait());
        assert!(future_b.as_mut().poll(&mut cx_b).is_pending());

        // set() should notify BOTH A and B (they were registered
        // when set was called), even though A's waker calls reset().
        event.set();

        // A was woken by the reentrant waker.
        assert!(waker_data_a.was_woken());

        // B should also have been notified despite the reentrant
        // reset(). Poll B to confirm.
        assert!(future_b.as_mut().poll(&mut cx_b).is_ready());
    }

    #[test]
    fn reentrant_drop_of_sibling_waiter_does_not_skip_others() {
        use std::cell::RefCell;
        use std::rc::Rc;

        use testing::ReentrantWakerData;

        // Register three futures A, B, C. A's waker drops future B (a
        // different sibling that is still waiting) and then resets the
        // event. With a snapshot-based set() implementation, B's
        // reentrant `drop_wait` would mutate `self.waiters` (then
        // empty) while B was still linked inside the snapshot —
        // leaving the snapshot in a corrupted state and causing C to
        // be lost. The rescan-from-head implementation tolerates this
        // because every iteration re-borrows the live `self.waiters`.
        //
        // The reentrant reset is what makes the bug observable in a
        // test: without it, `poll()` short-circuits on `is_set` and
        // returns Ready regardless of whether C was actually notified
        // by set().
        let event = LocalManualResetEvent::boxed();
        let event_for_waker = event.clone();

        let future_b_holder: Rc<RefCell<Option<Pin<Box<LocalManualResetWaitFuture>>>>> =
            Rc::new(RefCell::new(None));
        let holder_for_waker = Rc::clone(&future_b_holder);

        let waker_data_a = ReentrantWakerData::new(move || {
            // Drop B reentrantly. B is still in WAITING state and will
            // unregister itself from the awaiter set.
            drop(holder_for_waker.borrow_mut().take());
            // Reset so that polling C cannot short-circuit on is_set.
            event_for_waker.reset();
        });
        // SAFETY: Data outlives waker, single-threaded test.
        let waker_a = unsafe { waker_data_a.waker() };
        let mut cx_a = task::Context::from_waker(&waker_a);

        let noop = Waker::noop();
        let mut cx_noop = task::Context::from_waker(noop);

        // Register A, B, C in this order (head → tail).
        let mut future_a = Box::pin(event.wait());
        assert!(future_a.as_mut().poll(&mut cx_a).is_pending());

        let mut future_b = Box::pin(event.wait());
        assert!(future_b.as_mut().poll(&mut cx_noop).is_pending());
        *future_b_holder.borrow_mut() = Some(future_b);

        let mut future_c = Box::pin(event.wait());
        assert!(future_c.as_mut().poll(&mut cx_noop).is_pending());

        // set() notifies A first, A's waker drops B and resets the
        // event, then the loop must still observe C in the live set
        // and notify it.
        event.set();

        assert!(waker_data_a.was_woken());
        assert!(future_b_holder.borrow().is_none());
        // C must have been notified (lifecycle = NOTIFIED) even though
        // is_set is now false from the reentrant reset.
        assert!(future_c.as_mut().poll(&mut cx_noop).is_ready());
    }

    #[test]
    fn reentrant_drop_of_tail_sibling_does_not_skip_others() {
        use std::cell::RefCell;
        use std::rc::Rc;

        use testing::ReentrantWakerData;

        // Register three futures A, B, C. A's waker drops the TAIL
        // future C and resets the event. The drain loop must still
        // observe B in the live set and notify it.
        let event = LocalManualResetEvent::boxed();
        let event_for_waker = event.clone();

        let future_c_holder: Rc<RefCell<Option<Pin<Box<LocalManualResetWaitFuture>>>>> =
            Rc::new(RefCell::new(None));
        let holder_for_waker = Rc::clone(&future_c_holder);

        let waker_data_a = ReentrantWakerData::new(move || {
            drop(holder_for_waker.borrow_mut().take());
            event_for_waker.reset();
        });
        // SAFETY: Data outlives waker, single-threaded test.
        let waker_a = unsafe { waker_data_a.waker() };
        let mut cx_a = task::Context::from_waker(&waker_a);

        let noop = Waker::noop();
        let mut cx_noop = task::Context::from_waker(noop);

        let mut future_a = Box::pin(event.wait());
        assert!(future_a.as_mut().poll(&mut cx_a).is_pending());

        let mut future_b = Box::pin(event.wait());
        assert!(future_b.as_mut().poll(&mut cx_noop).is_pending());

        let mut future_c = Box::pin(event.wait());
        assert!(future_c.as_mut().poll(&mut cx_noop).is_pending());
        *future_c_holder.borrow_mut() = Some(future_c);

        event.set();

        assert!(waker_data_a.was_woken());
        assert!(future_c_holder.borrow().is_none());
        assert!(future_b.as_mut().poll(&mut cx_noop).is_ready());
    }

    #[test]
    fn reset_while_waiters_registered() {
        let event = LocalManualResetEvent::boxed();
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        let mut future = Box::pin(event.wait());
        assert!(future.as_mut().poll(&mut cx).is_pending());

        // Reset while a waiter is registered (gate was never open).
        event.reset();

        // The waiter is still pending.
        assert!(future.as_mut().poll(&mut cx).is_pending());

        // Now set — the waiter should be released.
        event.set();
        assert!(future.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn embedded_default_creates_unset_event() {
        let container = Box::pin(EmbeddedLocalManualResetEvent::default());
        // SAFETY: The container outlives the handle.
        let event = unsafe { LocalManualResetEvent::embedded(container.as_ref()) };
        assert!(!event.try_wait());
    }
}
