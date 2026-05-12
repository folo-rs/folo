use std::fmt;
use std::future::Future;
use std::marker::PhantomPinned;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{self, Poll, Waker};

use awaiter_set::{Awaiter, AwaiterSet};

use crate::NEVER_POISONED;

/// Thread-safe async manual-reset event.
///
/// Once set, releases all current and future awaiters until explicitly reset.
///
/// A `ManualResetEvent` acts as a gate: while set, every call to
/// [`wait()`][Self::wait] completes immediately. Calling [`reset()`][Self::reset]
/// closes the gate so that subsequent awaiters block until the next
/// [`set()`][Self::set].
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
/// # Re-entrancy
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
/// The event always releases its internal mutex before calling
/// [`Waker::wake()`], so re-entrant operations never deadlock or
/// observe partially mutated state.
///
/// # Examples
///
/// ```
/// use events::ManualResetEvent;
///
/// #[tokio::main]
/// async fn main() {
///     let event = ManualResetEvent::boxed();
///     let setter = event.clone();
///
///     // Producer opens the gate from a background task.
///     tokio::spawn(async move {
///         setter.set();
///     });
///
///     // Consumer waits for the gate to open.
///     event.wait().await;
///
///     // The gate stays open — it must be explicitly closed.
///     assert!(event.try_wait());
///
///     // Close the gate again.
///     event.reset();
///     assert!(!event.try_wait());
/// }
/// ```
#[derive(Clone)]
pub struct ManualResetEvent {
    inner: Arc<EventInner>,
}

const IS_SET: u8 = 0x1;
const HAS_WAITERS: u8 = 0x2;

struct EventInner {
    state: AtomicU8,
    slow: Mutex<AwaiterSet>,
}

impl EventInner {
    fn set(&self) {
        // Set IS_SET atomically. If HAS_WAITERS was not set, the
        // returned previous value will have HAS_WAITERS == 0 and
        // we are done. Using fetch_or instead of load+store avoids
        // a race where a concurrent waiter registration would set
        // HAS_WAITERS between our load and store.
        let prev = self.state.fetch_or(IS_SET, Ordering::Release);
        if prev & HAS_WAITERS == 0 {
            return;
        }

        // Slow path: waiters exist — drain all, waking each outside
        // the mutex to avoid deadlocks with re-entrant wakers.
        loop {
            let mut waiters = self.slow.lock().expect(NEVER_POISONED);
            let waker = waiters.notify_one();
            if let Some(w) = waker {
                if waiters.is_empty() {
                    self.state.fetch_and(!HAS_WAITERS, Ordering::Relaxed);
                }
                drop(waiters);
                w.wake();
            } else {
                self.state.fetch_and(!HAS_WAITERS, Ordering::Relaxed);
                break;
            }
        }
    }

    fn reset(&self) {
        self.state.fetch_and(!IS_SET, Ordering::Release);
    }

    fn try_wait(&self) -> bool {
        self.state.load(Ordering::Acquire) & IS_SET != 0
    }

    unsafe fn poll_wait(&self, awaiter: *mut Awaiter, waker: Waker) -> Poll<()> {
        // Fast path: event is already set.
        if self.state.load(Ordering::Acquire) & IS_SET != 0 {
            return Poll::Ready(());
        }

        // Check if we were directly notified by set() before taking the mutex.
        // Shared access to the awaiter only — the notifier may concurrently
        // hold its own shared reference under the event mutex.
        // SAFETY: The pointer references an awaiter pinned inside the
        // owning future, which outlives this call.
        let awaiter_ref = unsafe { &*awaiter };
        if awaiter_ref.take_notification() {
            return Poll::Ready(());
        }

        #[cfg(test)]
        crate::test_hooks::run(&crate::test_hooks::MANUAL_PRE_MUTEX);

        // Slow path: acquire the mutex.
        let mut waiters = self.slow.lock().expect(NEVER_POISONED);

        // Re-check notification under the mutex. A concurrent set() may
        // have taken the slow path and notified us before we acquired
        // the mutex.
        if awaiter_ref.take_notification() {
            return Poll::Ready(());
        }

        #[cfg(test)]
        crate::test_hooks::run(&crate::test_hooks::MANUAL_PRE_LOAD);

        // Re-check under the mutex. A concurrent set() may have taken its
        // fast path and stored IS_SET before we acquired the mutex.
        if self.state.load(Ordering::Acquire) & IS_SET != 0 {
            return Poll::Ready(());
        }

        #[cfg(test)]
        crate::test_hooks::run(&crate::test_hooks::MANUAL_PRE_FETCH_OR);

        // Register or update the waker. Set HAS_WAITERS before the
        // final check to close the race window with set().
        self.state.fetch_or(HAS_WAITERS, Ordering::Relaxed);

        // Re-check after setting HAS_WAITERS. A concurrent set() that
        // ran between the previous check and fetch_or would have stored
        // IS_SET via its fast path, which requires state==0, which in
        // turn requires the awaiter set to be empty. So when this branch
        // fires we are the only would-be waiter and can unconditionally
        // clear HAS_WAITERS.
        if self.state.load(Ordering::Acquire) & IS_SET != 0 {
            self.state.fetch_and(!HAS_WAITERS, Ordering::Relaxed);
            return Poll::Ready(());
        }

        // SAFETY: We hold the mutex, the pointer references a pinned
        // awaiter, and no other shared reference is live (the prior
        // `awaiter_ref` borrows are no longer used past this point).
        let awaiter_mut = unsafe { &mut *awaiter };
        // SAFETY: The awaiter is pinned inside the owning future.
        let awaiter_mut = unsafe { Pin::new_unchecked(awaiter_mut) };
        // SAFETY: We hold the mutex.
        unsafe {
            waiters.register(awaiter_mut, waker);
        }

        Poll::Pending
    }

    unsafe fn drop_wait(&self, awaiter: *mut Awaiter) {
        // SAFETY: The pointer references an awaiter pinned inside the
        // owning future, which outlives this call.
        let awaiter_ref = unsafe { &*awaiter };
        if !awaiter_ref.is_registered() {
            return;
        }

        let mut waiters = self.slow.lock().expect(NEVER_POISONED);

        // SAFETY: We hold the mutex, the pointer references a pinned
        // awaiter, and no other shared reference is live (the prior
        // `awaiter_ref` borrow is no longer used past this point).
        let awaiter_mut = unsafe { &mut *awaiter };
        // SAFETY: The awaiter is pinned inside the owning future.
        let awaiter_mut = unsafe { Pin::new_unchecked(awaiter_mut) };
        // SAFETY: We hold the mutex.
        unsafe {
            waiters.unregister(awaiter_mut);
        }
        if waiters.is_empty() {
            self.state.fetch_and(!HAS_WAITERS, Ordering::Relaxed);
        }
    }
}

impl ManualResetEvent {
    /// Creates a new event in the unset state.
    ///
    /// The state is heap-allocated. Clone the handle to share the same
    /// event. For caller-provided storage, see
    /// [`embedded()`][Self::embedded].
    ///
    /// # Examples
    ///
    /// ```
    /// use events::ManualResetEvent;
    ///
    /// let event = ManualResetEvent::boxed();
    /// let clone = event.clone();
    ///
    /// // Both handles operate on the same underlying event.
    /// clone.set();
    /// assert!(event.try_wait());
    /// ```
    #[must_use]
    pub fn boxed() -> Self {
        Self {
            inner: Arc::new(EventInner {
                state: AtomicU8::new(0),
                slow: Mutex::new(AwaiterSet::new()),
            }),
        }
    }

    /// Creates a handle from an [`EmbeddedManualResetEvent`] container,
    /// avoiding heap allocation.
    ///
    /// Calling this multiple times on the same container is safe and
    /// returns handles that all operate on the same shared state, just
    /// like copying or cloning a [`EmbeddedManualResetEventRef`].
    ///
    /// # Safety
    ///
    /// The caller must ensure that the [`EmbeddedManualResetEvent`] outlives
    /// all returned handles and any [`EmbeddedManualResetWaitFuture`]s created
    /// from them.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::pin::pin;
    ///
    /// use events::{EmbeddedManualResetEvent, ManualResetEvent};
    ///
    /// # futures::executor::block_on(async {
    /// let container = pin!(EmbeddedManualResetEvent::new());
    ///
    /// // SAFETY: The container outlives the handle and all wait futures.
    /// let event = unsafe { ManualResetEvent::embedded(container.as_ref()) };
    /// let setter = event;
    ///
    /// setter.set();
    /// event.wait().await;
    /// # });
    /// ```
    #[must_use]
    pub unsafe fn embedded(place: Pin<&EmbeddedManualResetEvent>) -> EmbeddedManualResetEventRef {
        let inner = NonNull::from(&place.get_ref().inner);
        EmbeddedManualResetEventRef { inner }
    }

    /// Opens the gate, releasing all current awaiters and allowing future
    /// awaiters to pass through immediately.
    ///
    /// If the event is already set, this is a no-op.
    ///
    /// # Examples
    ///
    /// ```
    /// use events::ManualResetEvent;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let event = ManualResetEvent::boxed();
    ///     let setter = event.clone();
    ///
    ///     tokio::spawn(async move {
    ///         setter.set();
    ///     });
    ///
    ///     event.wait().await;
    ///
    ///     // The gate stays open after waiting.
    ///     assert!(event.try_wait());
    /// }
    /// ```
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn set(&self) {
        self.inner.set();
    }

    /// Closes the gate. Future calls to [`wait()`][Self::wait] will block
    /// until the next [`set()`][Self::set].
    ///
    /// Awaiters that are already past the gate (i.e. whose futures have
    /// already returned [`Poll::Ready`]) are not affected.
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn reset(&self) {
        self.inner.reset();
    }

    /// Returns `true` if the event is currently set.
    ///
    /// Because other threads may set or reset the event concurrently, the
    /// returned value is immediately stale. Use this for diagnostics or
    /// best-effort checks, not for synchronization.
    #[must_use]
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn try_wait(&self) -> bool {
        self.inner.try_wait()
    }

    /// Returns a future that completes when the event is set.
    ///
    /// If the event is set at the time of polling, the future
    /// completes immediately. Once a [`set()`][Self::set] call has
    /// released this wait operation, the future remains ready even
    /// if [`reset()`][Self::reset] is called before the future is
    /// polled again.
    ///
    /// The returned future is `Send` and can be awaited on any
    /// thread.
    ///
    /// # Examples
    ///
    /// ```
    /// use events::ManualResetEvent;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let event = ManualResetEvent::boxed();
    ///     let setter = event.clone();
    ///
    ///     // Both tasks share the same event.
    ///     let w1 = event.clone();
    ///     let w2 = event.clone();
    ///
    ///     tokio::spawn(async move {
    ///         setter.set();
    ///     });
    ///
    ///     // All waiters complete once the gate is opened.
    ///     w1.wait().await;
    ///     w2.wait().await;
    ///
    ///     // The gate stays open — it does not auto-reset.
    ///     assert!(event.try_wait());
    /// }
    /// ```
    #[must_use]
    pub fn wait(&self) -> ManualResetWaitFuture {
        ManualResetWaitFuture {
            inner: Arc::clone(&self.inner),
            awaiter: Awaiter::new(),
        }
    }
}

/// Future returned by [`ManualResetEvent::wait()`].
///
/// Completes with `()` when the event is in the set state at the time of
/// polling.
pub struct ManualResetWaitFuture {
    inner: Arc<EventInner>,
    awaiter: Awaiter,
}

// Marker trait impl.
// SAFETY: Awaiter is Send. All awaiter access is protected by the event's
// Mutex. The Arc<EventInner> is Send + Sync.
unsafe impl Send for ManualResetWaitFuture {}

// Awaiter is UnwindSafe and RefUnwindSafe.
// Marker trait impl.
impl UnwindSafe for ManualResetWaitFuture {}
// Marker trait impl.
impl RefUnwindSafe for ManualResetWaitFuture {}

impl Future for ManualResetWaitFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<()> {
        let waker = cx.waker().clone();
        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };
        // Capture a raw pointer to the awaiter. No `&mut Awaiter` is
        // created here; the mutable reference is built later inside
        // the event mutex.
        let awaiter: *mut Awaiter = &raw mut this.awaiter;
        // SAFETY: The awaiter is pinned inside this future and outlives
        // the call; `inner.slow` is the mutex it registers with.
        unsafe { this.inner.poll_wait(awaiter, waker) }
    }
}

impl Drop for ManualResetWaitFuture {
    fn drop(&mut self) {
        // No `&mut Awaiter` is created here; the mutable reference is
        // built later inside the event mutex when needed.
        let awaiter: *mut Awaiter = &raw mut self.awaiter;
        // SAFETY: The awaiter is pinned inside this future and outlives
        // the call; the awaiter belongs to this event.
        unsafe { self.inner.drop_wait(awaiter) }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract for Debug format.
impl fmt::Debug for ManualResetEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let is_set = self.try_wait();
        f.debug_struct("ManualResetEvent")
            .field("is_set", &is_set)
            .finish()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract for Debug format.
impl fmt::Debug for ManualResetWaitFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ManualResetWaitFuture")
            // SAFETY: Debug output is best-effort; no concurrent
            // mutation during formatting.
            .finish_non_exhaustive()
    }
}

/// Embedded-state container for [`ManualResetEvent`].
///
/// Stores the event state inline in a struct, avoiding the heap allocation
/// that [`ManualResetEvent::boxed()`] requires. Create the container with
/// [`new()`][Self::new], pin it, then call [`ManualResetEvent::embedded()`]
/// to obtain a [`EmbeddedManualResetEventRef`] handle.
///
/// # Examples
///
/// ```
/// use std::pin::pin;
///
/// use events::{EmbeddedManualResetEvent, ManualResetEvent};
///
/// # futures::executor::block_on(async {
/// let container = pin!(EmbeddedManualResetEvent::new());
///
/// // SAFETY: The container outlives the handle and all wait futures.
/// let event = unsafe { ManualResetEvent::embedded(container.as_ref()) };
/// let waiter = event.clone();
///
/// event.set();
/// waiter.wait().await;
///
/// // The gate stays open — it must be explicitly closed.
/// assert!(event.try_wait());
/// # });
/// ```
pub struct EmbeddedManualResetEvent {
    inner: EventInner,
    _pinned: PhantomPinned,
}

impl EmbeddedManualResetEvent {
    /// Creates a new embedded event container in the unset state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: EventInner {
                state: AtomicU8::new(0),
                slow: Mutex::new(AwaiterSet::new()),
            },
            _pinned: PhantomPinned,
        }
    }
}

impl Default for EmbeddedManualResetEvent {
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder to new().
    fn default() -> Self {
        Self::new()
    }
}

/// Handle to an embedded [`ManualResetEvent`].
///
/// Created via [`ManualResetEvent::embedded()`]. The caller is responsible
/// for ensuring the [`EmbeddedManualResetEvent`] outlives all handles and
/// wait futures.
///
/// The API is identical to [`ManualResetEvent`].
#[derive(Clone, Copy)]
pub struct EmbeddedManualResetEventRef {
    inner: NonNull<EventInner>,
}

// Marker trait impl.
// SAFETY: EventInner is Send + Sync. The raw pointer is only dereferenced to
// obtain &EventInner, which is safe to share across threads.
unsafe impl Send for EmbeddedManualResetEventRef {}

// Marker trait impl.
// SAFETY: Same as Send — all mutable access is mediated by the Mutex.
unsafe impl Sync for EmbeddedManualResetEventRef {}

// Marker trait impl.
impl UnwindSafe for EmbeddedManualResetEventRef {}
// Marker trait impl.
impl RefUnwindSafe for EmbeddedManualResetEventRef {}

impl EmbeddedManualResetEventRef {
    fn inner(&self) -> &EventInner {
        // SAFETY: The caller of `embedded()` guarantees the container
        // outlives this handle.
        unsafe { self.inner.as_ref() }
    }

    /// Opens the gate, releasing all current awaiters.
    ///
    /// If the event is already set, this is a no-op.
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn set(&self) {
        self.inner().set();
    }

    /// Closes the gate.
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn reset(&self) {
        self.inner().reset();
    }

    /// Returns `true` if the event is currently set.
    #[must_use]
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn try_wait(&self) -> bool {
        self.inner().try_wait()
    }

    /// Returns a future that completes when the event is set.
    #[must_use]
    pub fn wait(&self) -> EmbeddedManualResetWaitFuture {
        EmbeddedManualResetWaitFuture {
            inner: self.inner,
            awaiter: Awaiter::new(),
        }
    }
}

/// Future returned by [`EmbeddedManualResetEventRef::wait()`].
pub struct EmbeddedManualResetWaitFuture {
    inner: NonNull<EventInner>,
    awaiter: Awaiter,
}

// Marker trait impl.
// SAFETY: Awaiter is Send. All awaiter access is protected by the event's
// Mutex.
unsafe impl Send for EmbeddedManualResetWaitFuture {}

// Marker trait impl.
impl UnwindSafe for EmbeddedManualResetWaitFuture {}
// Marker trait impl.
impl RefUnwindSafe for EmbeddedManualResetWaitFuture {}

impl Future for EmbeddedManualResetWaitFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<()> {
        let waker = cx.waker().clone();
        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };
        // SAFETY: The container outlives this future per the embedded()
        // contract.
        let inner = unsafe { this.inner.as_ref() };
        // Capture a raw pointer to the awaiter. No `&mut Awaiter` is
        // created here; the mutable reference is built later inside
        // the event mutex.
        let awaiter: *mut Awaiter = &raw mut this.awaiter;
        // SAFETY: The awaiter is pinned inside this future and outlives
        // the call; `inner.slow` is the mutex it registers with.
        unsafe { inner.poll_wait(awaiter, waker) }
    }
}

impl Drop for EmbeddedManualResetWaitFuture {
    fn drop(&mut self) {
        // SAFETY: The container outlives this future per the
        // embedded() contract.
        let inner = unsafe { self.inner.as_ref() };
        // No `&mut Awaiter` is created here; the mutable reference is
        // built later inside the event mutex when needed.
        let awaiter: *mut Awaiter = &raw mut self.awaiter;
        // SAFETY: The awaiter is pinned inside this future and outlives
        // the call; `inner.slow` is the mutex it was registered with.
        unsafe { inner.drop_wait(awaiter) }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for EmbeddedManualResetEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedManualResetEvent")
            .finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for EmbeddedManualResetEventRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let is_set = self.try_wait();
        f.debug_struct("EmbeddedManualResetEventRef")
            .field("is_set", &is_set)
            .finish()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for EmbeddedManualResetWaitFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedManualResetWaitFuture")
            // SAFETY: Debug output is best-effort; no concurrent
            // mutation during formatting.
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Barrier;
    use std::task::Waker;
    use std::{iter, thread};

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::test_hooks::BarrierHook;

    assert_impl_all!(ManualResetEvent: Send, Sync, Clone, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ManualResetWaitFuture: Send, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(ManualResetWaitFuture: Sync, Unpin);

    assert_impl_all!(EmbeddedManualResetEvent: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(EmbeddedManualResetEvent: Unpin);
    assert_impl_all!(
        EmbeddedManualResetEventRef: Send, Sync, Clone, Copy, UnwindSafe, RefUnwindSafe
    );
    assert_impl_all!(EmbeddedManualResetWaitFuture: Send, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(EmbeddedManualResetWaitFuture: Sync, Unpin);

    #[test]
    fn starts_unset() {
        let event = ManualResetEvent::boxed();
        assert!(!event.try_wait());
        assert!(!event.try_wait());
    }

    #[test]
    fn set_makes_is_set_true() {
        let event = ManualResetEvent::boxed();
        event.set();
        assert!(event.try_wait());
    }

    #[test]
    fn reset_after_set() {
        let event = ManualResetEvent::boxed();
        event.set();
        event.reset();
        assert!(!event.try_wait());
    }

    #[test]
    fn clone_shares_state() {
        let a = ManualResetEvent::boxed();
        let b = a.clone();
        a.set();
        assert!(b.try_wait());
    }

    #[test]
    fn wait_completes_when_already_set() {
        futures::executor::block_on(async {
            let event = ManualResetEvent::boxed();
            event.set();
            event.wait().await;
        });
    }

    #[test]
    fn wait_completes_after_set() {
        let event = ManualResetEvent::boxed();
        let mut future = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(future.as_mut().poll(&mut cx).is_pending());
        event.set();
        assert!(future.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn multiple_waiters_all_released() {
        let event = ManualResetEvent::boxed();

        let mut f1 = Box::pin(event.wait());
        let mut f2 = Box::pin(event.wait());
        let mut f3 = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());
        assert!(f3.as_mut().poll(&mut cx).is_pending());

        // Set releases all.
        event.set();

        assert!(f1.as_mut().poll(&mut cx).is_ready());
        assert!(f2.as_mut().poll(&mut cx).is_ready());
        assert!(f3.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn drop_future_while_waiting() {
        futures::executor::block_on(async {
            let event = ManualResetEvent::boxed();
            {
                let _future = event.wait();
            }
            event.set();
            event.wait().await;
        });
    }

    #[test]
    fn drop_polled_future_while_waiting() {
        let event = ManualResetEvent::boxed();
        let mut future = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        // Poll once to register in the awaiter set.
        assert!(future.as_mut().poll(&mut cx).is_pending());

        // Drop the registered future — should unlink cleanly.
        drop(future);

        // Event should still work after the cancelled waiter is gone.
        event.set();
        let mut future2 = Box::pin(event.wait());
        assert!(future2.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn set_wakes_registered_waiter() {
        use crate::test_helpers::AtomicWakeTracker;

        let event = ManualResetEvent::boxed();

        let tracker = AtomicWakeTracker::new();
        // SAFETY: The tracker outlives the waker.
        let waker = unsafe { tracker.waker() };
        let mut cx = task::Context::from_waker(&waker);

        let mut future = Box::pin(event.wait());
        assert!(future.as_mut().poll(&mut cx).is_pending());

        event.set();

        assert!(tracker.was_woken());
    }

    #[test]
    fn set_from_another_thread() {
        testing::with_watchdog(|| {
            let event = ManualResetEvent::boxed();
            let setter = event.clone();
            let barrier = Arc::new(Barrier::new(2));
            let b2 = Arc::clone(&barrier);

            let handle = thread::spawn(move || {
                b2.wait();
                setter.set();
            });

            barrier.wait();

            // Spin until set. This is acceptable in tests — the other thread
            // will set the event promptly after the barrier.
            while !event.try_wait() {
                std::hint::spin_loop();
            }

            handle.join().unwrap();
        });
    }

    #[test]
    fn multiple_waiters_from_different_threads() {
        testing::with_watchdog(|| {
            let event = ManualResetEvent::boxed();
            let waiter_count = 4;
            let barrier = Arc::new(Barrier::new(waiter_count + 1));
            let all_done = Arc::new(Barrier::new(waiter_count + 1));

            let handles: Vec<_> = iter::repeat_with(|| {
                let e = event.clone();
                let b = Arc::clone(&barrier);
                let done = Arc::clone(&all_done);

                thread::spawn(move || {
                    b.wait();

                    while !e.try_wait() {
                        std::hint::spin_loop();
                    }

                    done.wait();
                })
            })
            .take(waiter_count)
            .collect();

            barrier.wait();
            event.set();
            all_done.wait();

            for h in handles {
                h.join().unwrap();
            }
        });
    }

    #[test]
    fn set_reset_race_across_threads() {
        testing::with_watchdog(|| {
            let event = ManualResetEvent::boxed();
            let barrier = Arc::new(Barrier::new(3));

            let setter = event.clone();
            let b1 = Arc::clone(&barrier);
            let h1 = thread::spawn(move || {
                b1.wait();
                for _ in 0..100 {
                    setter.set();
                    std::hint::spin_loop();
                }
            });

            let resetter = event;
            let b2 = Arc::clone(&barrier);
            let h2 = thread::spawn(move || {
                b2.wait();
                for _ in 0..100 {
                    resetter.reset();
                    std::hint::spin_loop();
                }
            });

            barrier.wait();
            h1.join().unwrap();
            h2.join().unwrap();

            // No assertion on final state — the test validates that
            // concurrent set/reset does not cause data races.
        });
    }

    // The next three tests use the [`crate::test_hooks`] infrastructure
    // to deterministically exercise the race-resolution branches in
    // `poll_wait()` that would otherwise depend on thread interleaving.
    // Each test pauses the producer thread inside `poll_wait()` via a
    // barrier hook, performs the racing operation from the test thread,
    // then releases the producer. The producer's poll is guaranteed to
    // hit the targeted branch.

    #[test]
    fn poll_wait_post_mutex_take_notification_branch() {
        // Covers the post-mutex `take_notification()` → Ready branch.
        // Race: a concurrent `set()` notifies our awaiter between the
        // pre-mutex `take_notification()` check and the moment we
        // acquire the mutex.
        testing::with_watchdog(|| {
            let BarrierHook {
                entered,
                proceed,
                hook,
            } = crate::test_hooks::barrier_hook();
            crate::test_hooks::with_hook(&crate::test_hooks::MANUAL_PRE_MUTEX, hook, || {
                let event = ManualResetEvent::boxed();

                // First poll on the test thread registers the awaiter.
                let mut future = Box::pin(event.wait());
                let waker = Waker::noop();
                let mut cx = task::Context::from_waker(waker);
                assert!(future.as_mut().poll(&mut cx).is_pending());

                // Second poll on a separate thread will pause at the
                // hook after the pre-mutex `take_notification()` check
                // but before locking the mutex.
                let producer = thread::spawn(move || {
                    crate::test_hooks::HOOK_PARTICIPANT.with(|c| c.set(true));
                    let waker = Waker::noop();
                    let mut cx = task::Context::from_waker(waker);
                    future.as_mut().poll(&mut cx)
                });

                entered.wait();
                event.set();
                proceed.wait();

                assert!(producer.join().unwrap().is_ready());
            });
        });
    }

    #[test]
    fn poll_wait_post_mutex_load_branch() {
        // Covers the post-mutex signal-load → Ready branch. Race: a
        // concurrent `set()` stores IS_SET via its fast path between
        // our post-mutex `take_notification()` check and the post-mutex
        // signal re-check.
        testing::with_watchdog(|| {
            let BarrierHook {
                entered,
                proceed,
                hook,
            } = crate::test_hooks::barrier_hook();
            crate::test_hooks::with_hook(&crate::test_hooks::MANUAL_PRE_LOAD, hook, || {
                let event = ManualResetEvent::boxed();

                let producer = thread::spawn({
                    let event = event.clone();
                    move || {
                        crate::test_hooks::HOOK_PARTICIPANT.with(|c| c.set(true));
                        let mut future = Box::pin(event.wait());
                        let waker = Waker::noop();
                        let mut cx = task::Context::from_waker(waker);
                        future.as_mut().poll(&mut cx)
                    }
                });

                entered.wait();
                event.set();
                proceed.wait();

                assert!(producer.join().unwrap().is_ready());
                // ManualResetEvent stays set after the wait completes.
                assert!(event.try_wait());
            });
        });
    }

    #[test]
    fn poll_wait_post_fetch_or_load_branch() {
        // Covers the post-`fetch_or(HAS_WAITERS)` signal-load → Ready
        // branch. Race: a concurrent `set()` stores IS_SET via its
        // fast path between our post-mutex signal-load and the
        // `fetch_or`.
        testing::with_watchdog(|| {
            let BarrierHook {
                entered,
                proceed,
                hook,
            } = crate::test_hooks::barrier_hook();
            crate::test_hooks::with_hook(&crate::test_hooks::MANUAL_PRE_FETCH_OR, hook, || {
                let event = ManualResetEvent::boxed();

                let producer = thread::spawn({
                    let event = event.clone();
                    move || {
                        crate::test_hooks::HOOK_PARTICIPANT.with(|c| c.set(true));
                        let mut future = Box::pin(event.wait());
                        let waker = Waker::noop();
                        let mut cx = task::Context::from_waker(waker);
                        future.as_mut().poll(&mut cx)
                    }
                });

                entered.wait();
                event.set();
                proceed.wait();

                assert!(producer.join().unwrap().is_ready());
                assert!(event.try_wait());
            });
        });
    }

    #[test]
    fn embedded_set_from_another_thread() {
        testing::with_watchdog(|| {
            let container = Box::pin(EmbeddedManualResetEvent::new());
            // SAFETY: The container outlives all handles in this test.
            let event = unsafe { ManualResetEvent::embedded(container.as_ref()) };
            let setter = event;
            let barrier = Arc::new(Barrier::new(2));
            let b2 = Arc::clone(&barrier);

            let handle = thread::spawn(move || {
                b2.wait();
                setter.set();
            });

            barrier.wait();

            while !event.try_wait() {
                std::hint::spin_loop();
            }

            handle.join().unwrap();
        });
    }

    #[test]
    fn embedded_set_and_wait() {
        futures::executor::block_on(async {
            let container = Box::pin(EmbeddedManualResetEvent::new());
            // SAFETY: The container outlives the handle within this test.
            let event = unsafe { ManualResetEvent::embedded(container.as_ref()) };

            event.set();
            event.wait().await;
        });
    }

    #[test]
    fn embedded_clone_shares_state() {
        futures::executor::block_on(async {
            let container = Box::pin(EmbeddedManualResetEvent::new());
            // SAFETY: The container outlives the handle within this test.
            let a = unsafe { ManualResetEvent::embedded(container.as_ref()) };
            let b = a;

            a.set();
            assert!(b.try_wait());
            b.wait().await;
        });
    }

    #[test]
    fn embedded_reset_after_set() {
        let container = Box::pin(EmbeddedManualResetEvent::new());
        // SAFETY: The container outlives the handle within this test.
        let event = unsafe { ManualResetEvent::embedded(container.as_ref()) };

        event.set();
        event.reset();
        assert!(!event.try_wait());
    }

    #[test]
    fn embedded_drop_future_while_waiting() {
        futures::executor::block_on(async {
            let container = Box::pin(EmbeddedManualResetEvent::new());
            // SAFETY: The container outlives the handle within this test.
            let event = unsafe { ManualResetEvent::embedded(container.as_ref()) };

            {
                let _future = event.wait();
            }
            event.set();
            event.wait().await;
        });
    }

    #[test]
    fn embedded_wait_registers_then_completes() {
        let container = Box::pin(EmbeddedManualResetEvent::new());
        // SAFETY: The container outlives the handle within this test.
        let event = unsafe { ManualResetEvent::embedded(container.as_ref()) };

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
    fn embedded_drop_registered_future() {
        let container = Box::pin(EmbeddedManualResetEvent::new());
        // SAFETY: The container outlives the handle within this test.
        let event = unsafe { ManualResetEvent::embedded(container.as_ref()) };

        let mut future = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        // Poll to register.
        assert!(future.as_mut().poll(&mut cx).is_pending());

        // Drop the registered future — should unlink cleanly.
        drop(future);

        // Event should still work.
        event.set();
        let mut future2 = Box::pin(event.wait());
        assert!(future2.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn embedded_multiple_waiters_released() {
        let container = Box::pin(EmbeddedManualResetEvent::new());
        // SAFETY: The container outlives the handle within this test.
        let event = unsafe { ManualResetEvent::embedded(container.as_ref()) };

        let mut f1 = Box::pin(event.wait());
        let mut f2 = Box::pin(event.wait());
        let mut f3 = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        // All register as waiters.
        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());
        assert!(f3.as_mut().poll(&mut cx).is_pending());

        // Set releases all.
        event.set();

        assert!(f1.as_mut().poll(&mut cx).is_ready());
        assert!(f2.as_mut().poll(&mut cx).is_ready());
        assert!(f3.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn try_wait_returns_true_when_set() {
        let event = ManualResetEvent::boxed();
        event.set();
        assert!(event.try_wait());
    }

    #[test]
    fn embedded_try_wait_returns_false_when_unset() {
        let container = Box::pin(EmbeddedManualResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { ManualResetEvent::embedded(container.as_ref()) };
        assert!(!event.try_wait());
    }

    #[test]
    fn embedded_try_wait_returns_true_when_set() {
        let container = Box::pin(EmbeddedManualResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { ManualResetEvent::embedded(container.as_ref()) };
        event.set();
        assert!(event.try_wait());
    }

    #[test]
    fn embedded_set_wakes_registered_waiter() {
        use crate::test_helpers::AtomicWakeTracker;

        let container = Box::pin(EmbeddedManualResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { ManualResetEvent::embedded(container.as_ref()) };

        let tracker = AtomicWakeTracker::new();
        // SAFETY: The tracker outlives the waker.
        let waker = unsafe { tracker.waker() };
        let mut cx = task::Context::from_waker(&waker);

        let mut future = Box::pin(event.wait());
        assert!(future.as_mut().poll(&mut cx).is_pending());

        event.set();

        assert!(tracker.was_woken());
    }

    #[test]
    fn drop_unlinks_registered_waiter_from_list() {
        use crate::test_helpers::AtomicWakeTracker;

        let event = ManualResetEvent::boxed();

        let tracker1 = AtomicWakeTracker::new();
        // SAFETY: The tracker outlives the waker.
        let waker1 = unsafe { tracker1.waker() };
        let mut cx1 = task::Context::from_waker(&waker1);

        let tracker2 = AtomicWakeTracker::new();
        // SAFETY: The tracker outlives the waker.
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
        use crate::test_helpers::AtomicWakeTracker;

        let container = Box::pin(EmbeddedManualResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { ManualResetEvent::embedded(container.as_ref()) };

        let tracker1 = AtomicWakeTracker::new();
        // SAFETY: The tracker outlives the waker.
        let waker1 = unsafe { tracker1.waker() };
        let mut cx1 = task::Context::from_waker(&waker1);

        let tracker2 = AtomicWakeTracker::new();
        // SAFETY: The tracker outlives the waker.
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
        let event = ManualResetEvent::boxed();
        event.set();
        assert!(event.try_wait());

        // Second set() should be a no-op (early return).
        event.set();
        assert!(event.try_wait());
    }

    #[test]
    fn embedded_set_when_already_set_is_noop() {
        let container = Box::pin(EmbeddedManualResetEvent::new());

        // SAFETY: The container is pinned and outlives the handle.
        let event = unsafe { ManualResetEvent::embedded(container.as_ref()) };

        event.set();
        assert!(event.try_wait());

        event.set();
        assert!(event.try_wait());
    }

    #[test]
    fn reset_while_waiters_registered() {
        let event = ManualResetEvent::boxed();
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
        let container = Box::pin(EmbeddedManualResetEvent::default());
        // SAFETY: The container outlives the handle.
        let event = unsafe { ManualResetEvent::embedded(container.as_ref()) };
        assert!(!event.try_wait());
    }

    //
    // These tests use a custom waker that re-entrantly accesses the same
    // event when woken. They catch:
    //   * Aliased mutable access (`AwaiterSet` borrow held while invoking
    //     `Waker::wake()`) — Miri flags this as UB.
    //   * Lost wakes when a re-entrant call mutates the waiter set
    //     mid-drain (the bug PR 141 fixed in `LocalManualResetEvent`).

    #[test]
    fn set_with_reentrant_waker_does_not_alias() {
        use testing::ReentrantWakerData;

        let event = ManualResetEvent::boxed();
        let event_clone = event.clone();

        let waker_data = ReentrantWakerData::new(move || {
            // Re-entrantly reset and poll a new waiter, which acquires
            // the mutex to register another awaiter.
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

        event.set();

        assert!(waker_data.was_woken());
    }

    #[test]
    fn reentrant_reset_does_not_skip_awaiters() {
        use testing::ReentrantWakerData;

        let event = ManualResetEvent::boxed();
        let event_for_waker = event.clone();

        let waker_data_a = ReentrantWakerData::new(move || {
            event_for_waker.reset();
        });
        // SAFETY: Data outlives waker, single-threaded test.
        let waker_a = unsafe { waker_data_a.waker() };
        let mut cx_a = task::Context::from_waker(&waker_a);

        let noop = Waker::noop();
        let mut cx_b = task::Context::from_waker(noop);

        let mut future_a = Box::pin(event.wait());
        assert!(future_a.as_mut().poll(&mut cx_a).is_pending());

        let mut future_b = Box::pin(event.wait());
        assert!(future_b.as_mut().poll(&mut cx_b).is_pending());

        // set() must notify BOTH A and B even though A's waker calls
        // reset(). B was registered when set was called and must not
        // be lost.
        event.set();

        assert!(waker_data_a.was_woken());
        assert!(future_b.as_mut().poll(&mut cx_b).is_ready());
    }

    #[test]
    fn reentrant_drop_of_middle_sibling_does_not_skip_others() {
        use std::cell::RefCell;
        use std::rc::Rc;

        use testing::ReentrantWakerData;

        // Register three futures A, B, C. A's waker drops future B
        // (the middle of three) and resets the event. The drain loop
        // must still observe C in the live set and notify it.
        let event = ManualResetEvent::boxed();
        let event_for_waker = event.clone();

        let future_b_holder: Rc<RefCell<Option<Pin<Box<ManualResetWaitFuture>>>>> =
            Rc::new(RefCell::new(None));
        let holder_for_waker = Rc::clone(&future_b_holder);

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
        *future_b_holder.borrow_mut() = Some(future_b);

        let mut future_c = Box::pin(event.wait());
        assert!(future_c.as_mut().poll(&mut cx_noop).is_pending());

        event.set();

        assert!(waker_data_a.was_woken());
        assert!(future_b_holder.borrow().is_none());
        // The re-entrant reset cleared `is_set`, so C must have been
        // notified directly by the drain loop.
        assert!(future_c.as_mut().poll(&mut cx_noop).is_ready());
    }

    #[test]
    fn reentrant_drop_of_tail_sibling_does_not_skip_others() {
        use std::cell::RefCell;
        use std::rc::Rc;

        use testing::ReentrantWakerData;

        // Register three futures A, B, C. A's waker drops future C
        // (the tail) and resets the event. The drain loop must still
        // observe B in the live set and notify it.
        let event = ManualResetEvent::boxed();
        let event_for_waker = event.clone();

        let future_c_holder: Rc<RefCell<Option<Pin<Box<ManualResetWaitFuture>>>>> =
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

        let future_c = Box::pin(event.wait());
        let mut future_c = future_c;
        assert!(future_c.as_mut().poll(&mut cx_noop).is_pending());
        *future_c_holder.borrow_mut() = Some(future_c);

        event.set();

        assert!(waker_data_a.was_woken());
        assert!(future_c_holder.borrow().is_none());
        assert!(future_b.as_mut().poll(&mut cx_noop).is_ready());
    }
}
