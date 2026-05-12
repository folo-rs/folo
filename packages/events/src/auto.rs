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

/// Thread-safe async auto-reset event.
///
/// Each [`set()`][Self::set] call releases at most one awaiter.
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
/// # Examples
///
/// ```
/// use events::AutoResetEvent;
///
/// #[tokio::main]
/// async fn main() {
///     let event = AutoResetEvent::boxed();
///     let setter = event.clone();
///
///     // Producer signals from a background task.
///     tokio::spawn(async move {
///         setter.set();
///     });
///
///     // Consumer waits for the signal.
///     event.wait().await;
///
///     // Signal was consumed.
///     assert!(!event.try_wait());
/// }
/// ```
#[derive(Clone)]
pub struct AutoResetEvent {
    inner: Arc<EventInner>,
}

const SIGNAL: u8 = 0x1;
const HAS_WAITERS: u8 = 0x2;

struct EventInner {
    state: AtomicU8,
    slow: Mutex<AwaiterSet>,
}

fn set(inner: &EventInner) {
    // Fast path: no waiters — set the signal atomically.
    if inner
        .state
        .compare_exchange(0, SIGNAL, Ordering::Release, Ordering::Relaxed)
        .is_ok()
    {
        return;
    }

    // If already set, nothing to do.
    let prev = inner.state.load(Ordering::Relaxed);
    if prev & SIGNAL != 0 {
        return;
    }

    // Slow path: waiters exist — notify one under the mutex.
    let waker: Option<Waker>;
    {
        let mut waiters = inner.slow.lock().expect(NEVER_POISONED);

        // SAFETY: We hold the mutex.
        if let Some(w) = unsafe { waiters.notify_one() } {
            // SAFETY: We hold the mutex.
            if unsafe { waiters.is_empty() } {
                inner.state.fetch_and(!HAS_WAITERS, Ordering::Relaxed);
            }
            waker = Some(w);
        } else {
            // No waiters despite HAS_WAITERS — set signal.
            inner.state.store(SIGNAL, Ordering::Release);
            waker = None;
        }
    }

    if let Some(w) = waker {
        w.wake();
    }
}

fn try_wait(inner: &EventInner) -> bool {
    // Atomically clear the SIGNAL bit, leaving HAS_WAITERS untouched.
    // Using fetch_and rather than compare_exchange(SIGNAL, 0) ensures
    // we still consume the signal when HAS_WAITERS happens to be set
    // (which can occur in the slow path of poll_wait after fetch_or).
    inner.state.fetch_and(!SIGNAL, Ordering::Acquire) & SIGNAL != 0
}

unsafe fn poll_wait(inner: &EventInner, mut awaiter: Pin<&mut Awaiter>, waker: Waker) -> Poll<()> {
    // Fast path: try to consume the signal atomically.
    if try_wait(inner) {
        return Poll::Ready(());
    }

    // Check if we were directly notified by set() before taking the mutex.
    if awaiter.as_ref().take_notification() {
        return Poll::Ready(());
    }

    #[cfg(test)]
    crate::test_hooks::run(&crate::test_hooks::AUTO_PRE_MUTEX);

    // Slow path: acquire the mutex.
    let mut waiters = inner.slow.lock().expect(NEVER_POISONED);

    // Re-check notification under the mutex. A concurrent set() may
    // have taken the slow path and notified us before we acquired
    // the mutex.
    if awaiter.as_ref().take_notification() {
        return Poll::Ready(());
    }

    #[cfg(test)]
    crate::test_hooks::run(&crate::test_hooks::AUTO_PRE_TRY_WAIT);

    // Re-check signal under the mutex. A concurrent set() may have
    // taken its fast path and stored SIGNAL before we acquired the
    // mutex.
    if try_wait(inner) {
        return Poll::Ready(());
    }

    #[cfg(test)]
    crate::test_hooks::run(&crate::test_hooks::AUTO_PRE_FETCH_OR);

    // Register or update the waker. Set HAS_WAITERS before the
    // final signal check to close the race window: a concurrent
    // set() that observes HAS_WAITERS will enter the slow path
    // and wake us. If set() runs between our try_wait and this
    // fetch_or, the re-check below catches it.
    inner.state.fetch_or(HAS_WAITERS, Ordering::Relaxed);

    // Re-check signal after setting HAS_WAITERS. A concurrent
    // set() that ran between try_wait and fetch_or would have
    // stored SIGNAL via its fast path, which requires state==0,
    // which in turn requires the awaiter set to be empty. So when
    // this branch fires we are the only would-be waiter and can
    // unconditionally clear HAS_WAITERS.
    if try_wait(inner) {
        inner.state.fetch_and(!HAS_WAITERS, Ordering::Relaxed);
        return Poll::Ready(());
    }

    // SAFETY: We hold the mutex, awaiter is pinned.
    unsafe {
        waiters.register(awaiter.as_mut(), waker);
    }
    Poll::Pending
}

unsafe fn drop_wait(inner: &EventInner, mut awaiter: Pin<&mut Awaiter>) {
    if !awaiter.is_registered() {
        return;
    }

    let mut waiters = inner.slow.lock().expect(NEVER_POISONED);

    if awaiter.as_ref().is_notified() {
        // We were notified but the future was cancelled. Forward
        // the notification to the next waiter.
        // SAFETY: We hold the mutex.
        if let Some(waker) = unsafe { waiters.notify_one() } {
            // SAFETY: We hold the mutex.
            if unsafe { waiters.is_empty() } {
                inner.state.fetch_and(!HAS_WAITERS, Ordering::Relaxed);
            }
            drop(waiters);
            waker.wake();
        } else {
            // No more waiters — restore the signal.
            inner.state.store(SIGNAL, Ordering::Release);
        }
    } else {
        // Not notified — just remove from the set.
        // SAFETY: We hold the mutex.
        unsafe {
            waiters.unregister(awaiter.as_mut());
        }
        // SAFETY: We hold the mutex.
        if unsafe { waiters.is_empty() } {
            inner.state.fetch_and(!HAS_WAITERS, Ordering::Relaxed);
        }
    }
}

impl AutoResetEvent {
    /// Creates a new event in the unset state.
    ///
    /// The state is heap-allocated. Clone the handle to share the same
    /// event. For caller-provided storage, see
    /// [`embedded()`][Self::embedded].
    ///
    /// # Examples
    ///
    /// ```
    /// use events::AutoResetEvent;
    ///
    /// let event = AutoResetEvent::boxed();
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

    /// Creates a handle from an [`EmbeddedAutoResetEvent`] container,
    /// avoiding heap allocation.
    ///
    /// Calling this multiple times on the same container is safe and
    /// returns handles that all operate on the same shared state, just
    /// like copying or cloning a [`EmbeddedAutoResetEventRef`].
    ///
    /// # Safety
    ///
    /// The caller must ensure that the [`EmbeddedAutoResetEvent`] outlives
    /// all returned handles and any [`EmbeddedAutoResetWaitFuture`]s created
    /// from them.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::pin::pin;
    ///
    /// use events::{AutoResetEvent, EmbeddedAutoResetEvent};
    ///
    /// # futures::executor::block_on(async {
    /// let container = pin!(EmbeddedAutoResetEvent::new());
    ///
    /// // SAFETY: The container outlives the handle and all wait futures.
    /// let event = unsafe { AutoResetEvent::embedded(container.as_ref()) };
    /// let setter = event;
    ///
    /// setter.set();
    /// event.wait().await;
    /// # });
    /// ```
    #[must_use]
    pub unsafe fn embedded(place: Pin<&EmbeddedAutoResetEvent>) -> EmbeddedAutoResetEventRef {
        let inner = NonNull::from(&place.get_ref().inner);
        EmbeddedAutoResetEventRef { inner }
    }

    /// Signals the event, releasing at most one waiter.
    ///
    /// * If one or more waiters are registered, a single waiter is
    ///   released and the event remains unset.
    /// * If no one is waiting, the event transitions to the set state
    ///   so that the next [`wait()`][Self::wait] or
    ///   [`try_wait()`][Self::try_wait] completes immediately.
    ///
    /// # Examples
    ///
    /// ```
    /// use events::AutoResetEvent;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let event = AutoResetEvent::boxed();
    ///     let setter = event.clone();
    ///
    ///     tokio::spawn(async move {
    ///         setter.set();
    ///     });
    ///
    ///     event.wait().await;
    /// }
    /// ```
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn set(&self) {
        set(&self.inner);
    }

    /// Attempts to consume the signal without blocking.
    ///
    /// Returns `true` if the event was set, atomically transitioning it back
    /// to the unset state. Returns `false` if the event was not set.
    ///
    /// # Examples
    ///
    /// ```
    /// use events::AutoResetEvent;
    ///
    /// let event = AutoResetEvent::boxed();
    /// assert!(!event.try_wait());
    ///
    /// event.set();
    /// assert!(event.try_wait());
    ///
    /// // Signal was consumed.
    /// assert!(!event.try_wait());
    /// ```
    #[must_use]
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn try_wait(&self) -> bool {
        try_wait(&self.inner)
    }

    /// Returns a future that completes when the event is signaled.
    ///
    /// When [`set()`][Self::set] is called, a single waiting future is
    /// released. If the event is already set (no prior waiter consumed it),
    /// the future completes immediately and consumes the signal.
    ///
    /// # Examples
    ///
    /// ```
    /// use events::AutoResetEvent;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let event = AutoResetEvent::boxed();
    ///     let setter = event.clone();
    ///
    ///     tokio::spawn(async move {
    ///         setter.set();
    ///     });
    ///
    ///     event.wait().await;
    /// }
    /// ```
    #[must_use]
    pub fn wait(&self) -> AutoResetWaitFuture {
        AutoResetWaitFuture {
            inner: Arc::clone(&self.inner),
            awaiter: Awaiter::new(),
        }
    }
}

/// Future returned by [`AutoResetEvent::wait()`].
///
/// Completes with `()` when the event signal is acquired.
pub struct AutoResetWaitFuture {
    inner: Arc<EventInner>,
    awaiter: Awaiter,
}

// Marker trait impl.
// SAFETY: Awaiter is Send. All awaiter access is protected by the event's
// Mutex. The Arc<EventInner> is Send + Sync.
unsafe impl Send for AutoResetWaitFuture {}

// Awaiter is UnwindSafe and RefUnwindSafe.
// Marker trait impl.
impl UnwindSafe for AutoResetWaitFuture {}
// Marker trait impl.
impl RefUnwindSafe for AutoResetWaitFuture {}

impl Future for AutoResetWaitFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<()> {
        // Clone the waker before acquiring the lock so a panicking clone
        // implementation cannot poison the mutex.
        let waker = cx.waker().clone();

        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };

        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut this.awaiter) };
        // SAFETY: The state field is the mutex this awaiter registers
        // with.
        unsafe { poll_wait(&this.inner, awaiter, waker) }
    }
}

impl Drop for AutoResetWaitFuture {
    fn drop(&mut self) {
        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut self.awaiter) };
        // SAFETY: The state field is the mutex this awaiter was
        // registered with.
        unsafe { drop_wait(&self.inner, awaiter) }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract for Debug format.
impl fmt::Debug for AutoResetEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AutoResetEvent").finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract for Debug format.
impl fmt::Debug for AutoResetWaitFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AutoResetWaitFuture")
            // SAFETY: Debug output is best-effort; no concurrent
            // mutation during formatting.
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Embedded variant
// ---------------------------------------------------------------------------

/// Embedded-state container for [`AutoResetEvent`].
///
/// Stores the event state inline in a struct, avoiding the heap allocation
/// that [`AutoResetEvent::boxed()`] requires. Create the container with
/// [`new()`][Self::new], pin it, then call [`AutoResetEvent::embedded()`]
/// to obtain a [`EmbeddedAutoResetEventRef`] handle.
///
/// # Examples
///
/// ```
/// use std::pin::pin;
///
/// use events::{AutoResetEvent, EmbeddedAutoResetEvent};
///
/// # futures::executor::block_on(async {
/// let container = pin!(EmbeddedAutoResetEvent::new());
///
/// // SAFETY: The container outlives the handle and all wait futures.
/// let event = unsafe { AutoResetEvent::embedded(container.as_ref()) };
/// let setter = event;
///
/// setter.set();
/// event.wait().await;
/// # });
/// ```
pub struct EmbeddedAutoResetEvent {
    inner: EventInner,
    _pinned: PhantomPinned,
}

impl EmbeddedAutoResetEvent {
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

impl Default for EmbeddedAutoResetEvent {
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder to new().
    fn default() -> Self {
        Self::new()
    }
}

/// Handle to an embedded [`AutoResetEvent`].
///
/// Created via [`AutoResetEvent::embedded()`]. The caller is responsible
/// for ensuring the [`EmbeddedAutoResetEvent`] outlives all handles and
/// wait futures.
///
/// The API is identical to [`AutoResetEvent`].
#[derive(Clone, Copy)]
pub struct EmbeddedAutoResetEventRef {
    inner: NonNull<EventInner>,
}

// Marker trait impl.
// SAFETY: EventInner is Send + Sync. The raw pointer is only dereferenced
// to obtain &EventInner, which is safe to share across threads.
unsafe impl Send for EmbeddedAutoResetEventRef {}

// Marker trait impl.
// SAFETY: Same as Send — all mutable access is mediated by the Mutex.
unsafe impl Sync for EmbeddedAutoResetEventRef {}

// Marker trait impl.
impl UnwindSafe for EmbeddedAutoResetEventRef {}
// Marker trait impl.
impl RefUnwindSafe for EmbeddedAutoResetEventRef {}

impl EmbeddedAutoResetEventRef {
    fn inner(&self) -> &EventInner {
        // SAFETY: The caller of `embedded()` guarantees the container
        // outlives this handle.
        unsafe { self.inner.as_ref() }
    }

    /// Signals the event, releasing exactly one waiter.
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn set(&self) {
        set(self.inner());
    }

    /// Attempts to consume the signal without blocking.
    ///
    /// Returns `true` if the event was set, atomically transitioning it
    /// back to the unset state. Returns `false` if the event was not set.
    #[must_use]
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn try_wait(&self) -> bool {
        try_wait(self.inner())
    }

    /// Returns a future that completes when the event is signaled.
    #[must_use]
    pub fn wait(&self) -> EmbeddedAutoResetWaitFuture {
        EmbeddedAutoResetWaitFuture {
            inner: self.inner,
            awaiter: Awaiter::new(),
        }
    }
}

/// Future returned by [`EmbeddedAutoResetEventRef::wait()`].
pub struct EmbeddedAutoResetWaitFuture {
    inner: NonNull<EventInner>,
    awaiter: Awaiter,
}

// Marker trait impl.
// SAFETY: Awaiter is Send. All awaiter access is protected by the event's
// Mutex.
unsafe impl Send for EmbeddedAutoResetWaitFuture {}

// Marker trait impl.
impl UnwindSafe for EmbeddedAutoResetWaitFuture {}
// Marker trait impl.
impl RefUnwindSafe for EmbeddedAutoResetWaitFuture {}

impl Future for EmbeddedAutoResetWaitFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<()> {
        // Clone the waker before acquiring the lock so a panicking clone
        // implementation cannot poison the mutex.
        let waker = cx.waker().clone();

        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };

        // SAFETY: The container outlives this future per the embedded()
        // contract.
        let inner = unsafe { this.inner.as_ref() };
        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut this.awaiter) };
        // SAFETY: The state is the mutex this awaiter registers with.
        unsafe { poll_wait(inner, awaiter, waker) }
    }
}

impl Drop for EmbeddedAutoResetWaitFuture {
    fn drop(&mut self) {
        // SAFETY: The container outlives this future per the embedded()
        // contract.
        let inner = unsafe { self.inner.as_ref() };
        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut self.awaiter) };
        // SAFETY: The state is the mutex this awaiter was registered
        // with.
        unsafe { drop_wait(inner, awaiter) }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for EmbeddedAutoResetEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedAutoResetEvent")
            .finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for EmbeddedAutoResetEventRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedAutoResetEventRef")
            .finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for EmbeddedAutoResetWaitFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedAutoResetWaitFuture")
            // SAFETY: Debug output is best-effort; no concurrent
            // mutation during formatting.
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::{iter, thread};

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::test_hooks::BarrierHook;

    // --- trait assertions ---

    assert_impl_all!(AutoResetEvent: Send, Sync, Clone, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(AutoResetWaitFuture: Send, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(AutoResetWaitFuture: Sync, Unpin);

    assert_impl_all!(EmbeddedAutoResetEvent: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(EmbeddedAutoResetEvent: Unpin);
    assert_impl_all!(EmbeddedAutoResetEventRef: Send, Sync, Clone, Copy, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(EmbeddedAutoResetWaitFuture: Send, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(EmbeddedAutoResetWaitFuture: Sync, Unpin);

    // --- basic functionality ---

    #[test]
    fn starts_unset() {
        let event = AutoResetEvent::boxed();
        assert!(!event.try_wait());
    }

    #[test]
    fn set_then_try_wait() {
        let event = AutoResetEvent::boxed();
        event.set();
        assert!(event.try_wait());
        // Signal consumed.
        assert!(!event.try_wait());
    }

    #[test]
    fn clone_shares_state() {
        let a = AutoResetEvent::boxed();
        let b = a.clone();
        a.set();
        assert!(b.try_wait());
    }

    #[test]
    fn double_set_without_waiter_only_stores_one_signal() {
        let event = AutoResetEvent::boxed();
        event.set();
        event.set();
        assert!(event.try_wait());
        // Second set was a no-op (already set).
        assert!(!event.try_wait());
    }

    // --- async tests ---

    #[test]
    fn wait_completes_when_already_set() {
        futures::executor::block_on(async {
            let event = AutoResetEvent::boxed();
            event.set();
            event.wait().await;
            // Signal consumed.
            assert!(!event.try_wait());
        });
    }

    #[test]
    fn wait_completes_after_set() {
        let event = AutoResetEvent::boxed();
        let mut future = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(future.as_mut().poll(&mut cx).is_pending());
        event.set();
        assert!(future.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn only_one_waiter_released_per_set() {
        let event = AutoResetEvent::boxed();

        let mut f1 = Box::pin(event.wait());
        let mut f2 = Box::pin(event.wait());
        let mut f3 = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        // All three register.
        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());
        assert!(f3.as_mut().poll(&mut cx).is_pending());

        // Signal once — exactly one waiter should complete.
        event.set();
        let mut ready_count = 0_u32;
        for f in [f1.as_mut(), f2.as_mut(), f3.as_mut()] {
            if f.poll(&mut cx).is_ready() {
                ready_count = ready_count.checked_add(1).unwrap();
            }
        }
        assert_eq!(ready_count, 1);

        // Signal twice more to release the remaining two.
        event.set();
        event.set();
        for f in [f1.as_mut(), f2.as_mut(), f3.as_mut()] {
            if f.poll(&mut cx).is_ready() {
                ready_count = ready_count.checked_add(1).unwrap();
            }
        }
        assert_eq!(ready_count, 3);
    }

    #[test]
    fn cancelled_waiter_forwards_notification() {
        let event = AutoResetEvent::boxed();

        let mut future = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        // Register the waiter.
        assert!(future.as_mut().poll(&mut cx).is_pending());

        // Drop without completing — cancellation should not lose the signal.
        drop(future);

        // Set and wait again — should work because the cancelled waiter
        // was never notified.
        event.set();
        let mut future2 = Box::pin(event.wait());
        assert!(future2.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn drop_unpolled_future_is_safe() {
        let event = AutoResetEvent::boxed();
        {
            let _future = event.wait();
        }
        event.set();
        futures::executor::block_on(event.wait());
    }

    // --- multithreaded tests (Miri-compatible) ---

    #[test]
    fn set_from_another_thread() {
        testing::with_watchdog(|| {
            let event = AutoResetEvent::boxed();
            let setter = event.clone();
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
    fn only_one_thread_acquires_signal() {
        testing::with_watchdog(|| {
            let event = AutoResetEvent::boxed();
            let waiter_count = 4;
            let barrier = Arc::new(Barrier::new(waiter_count + 1));
            let acquired_count = Arc::new(AtomicUsize::new(0));

            let handles: Vec<_> = iter::repeat_with(|| {
                let e = event.clone();
                let b = Arc::clone(&barrier);
                let count = Arc::clone(&acquired_count);

                thread::spawn(move || {
                    b.wait();

                    // Each thread tries to acquire many times.
                    for _ in 0..200 {
                        if e.try_wait() {
                            count.fetch_add(1, Ordering::Relaxed);
                        }
                        std::hint::spin_loop();
                    }
                })
            })
            .take(waiter_count)
            .collect();

            // Set before releasing threads so the signal is guaranteed
            // to be available when they start competing.
            event.set();
            barrier.wait();

            for h in handles {
                h.join().unwrap();
            }

            // Exactly one thread should have acquired the single signal.
            assert_eq!(acquired_count.load(Ordering::Relaxed), 1);
        });
    }

    #[test]
    fn multiple_sets_release_multiple_threads() {
        testing::with_watchdog(|| {
            let event = AutoResetEvent::boxed();
            let signal_count = 4;
            let barrier = Arc::new(Barrier::new(signal_count + 1));
            let acquired_count = Arc::new(AtomicUsize::new(0));

            let handles: Vec<_> = iter::repeat_with(|| {
                let e = event.clone();
                let b = Arc::clone(&barrier);
                let count = Arc::clone(&acquired_count);

                thread::spawn(move || {
                    b.wait();

                    // Spin until we acquire a signal.
                    while !e.try_wait() {
                        std::hint::spin_loop();
                    }

                    count.fetch_add(1, Ordering::Relaxed);
                })
            })
            .take(signal_count)
            .collect();

            barrier.wait();

            // Keep setting until all threads have acquired a signal.
            // Each set() stores at most one signal, so we must set
            // repeatedly rather than calling set() N times in a row.
            while acquired_count.load(Ordering::Relaxed) < signal_count {
                event.set();
                std::hint::spin_loop();
            }

            for h in handles {
                h.join().unwrap();
            }
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
            crate::test_hooks::with_hook(&crate::test_hooks::AUTO_PRE_MUTEX, hook, || {
                let event = AutoResetEvent::boxed();

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
    fn poll_wait_post_mutex_try_wait_branch() {
        // Covers the post-mutex `try_wait()` → Ready branch. Race: a
        // concurrent `set()` stores SIGNAL via its fast path between
        // our post-mutex `take_notification()` check and the post-mutex
        // signal re-check.
        testing::with_watchdog(|| {
            let BarrierHook {
                entered,
                proceed,
                hook,
            } = crate::test_hooks::barrier_hook();
            crate::test_hooks::with_hook(&crate::test_hooks::AUTO_PRE_TRY_WAIT, hook, || {
                let event = AutoResetEvent::boxed();

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
                // The signal was consumed by the producer.
                assert!(!event.try_wait());
            });
        });
    }

    #[test]
    fn poll_wait_post_fetch_or_try_wait_branch() {
        // Covers the post-`fetch_or(HAS_WAITERS)` `try_wait()` → Ready
        // branch. Regression coverage for the previously-fixed CAS bug.
        // Race: a concurrent `set()` stores SIGNAL via its fast path
        // between our post-mutex `try_wait()` check and the `fetch_or`.
        testing::with_watchdog(|| {
            let BarrierHook {
                entered,
                proceed,
                hook,
            } = crate::test_hooks::barrier_hook();
            crate::test_hooks::with_hook(&crate::test_hooks::AUTO_PRE_FETCH_OR, hook, || {
                let event = AutoResetEvent::boxed();

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
                assert!(!event.try_wait());
            });
        });
    }

    #[test]
    fn await_races_with_set_across_threads() {
        // Regression test for a race where poll_wait() observed
        // HAS_WAITERS|SIGNAL state after fetch_or, but the CAS-based
        // try_wait failed because it required exact match on SIGNAL.
        // Many awaiters waited forever despite set() running. Each
        // iteration creates a real future and awaits it while a
        // separate thread calls set() concurrently.
        testing::with_watchdog(|| {
            const ITERATIONS: usize = 200;

            let event = AutoResetEvent::boxed();

            for _ in 0..ITERATIONS {
                let barrier = Arc::new(Barrier::new(2));

                let setter_handle = thread::spawn({
                    let event = event.clone();
                    let barrier = Arc::clone(&barrier);
                    move || {
                        barrier.wait();
                        event.set();
                    }
                });

                let waiter_handle = thread::spawn({
                    let event = event.clone();
                    let barrier = Arc::clone(&barrier);
                    move || {
                        barrier.wait();
                        futures::executor::block_on(event.wait());
                    }
                });

                setter_handle.join().unwrap();
                waiter_handle.join().unwrap();
            }
        });
    }

    #[test]
    fn embedded_set_from_another_thread() {
        testing::with_watchdog(|| {
            let container = Box::pin(EmbeddedAutoResetEvent::new());
            // SAFETY: The container outlives all handles in this test.
            let event = unsafe { AutoResetEvent::embedded(container.as_ref()) };
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

    // --- embedded variant tests ---

    #[test]
    fn embedded_set_and_wait() {
        futures::executor::block_on(async {
            let container = Box::pin(EmbeddedAutoResetEvent::new());
            // SAFETY: The container outlives the handle within this test.
            let event = unsafe { AutoResetEvent::embedded(container.as_ref()) };

            event.set();
            event.wait().await;
        });
    }

    #[test]
    fn embedded_clone_shares_state() {
        let container = Box::pin(EmbeddedAutoResetEvent::new());
        // SAFETY: The container outlives the handle within this test.
        let a = unsafe { AutoResetEvent::embedded(container.as_ref()) };
        let b = a;

        a.set();
        assert!(b.try_wait());
    }

    #[test]
    fn embedded_signal_consumed() {
        let container = Box::pin(EmbeddedAutoResetEvent::new());
        // SAFETY: The container outlives the handle within this test.
        let event = unsafe { AutoResetEvent::embedded(container.as_ref()) };

        event.set();
        assert!(event.try_wait());
        // Signal was consumed.
        assert!(!event.try_wait());
    }

    #[test]
    fn embedded_drop_future_while_waiting() {
        futures::executor::block_on(async {
            let container = Box::pin(EmbeddedAutoResetEvent::new());
            // SAFETY: The container outlives the handle within this test.
            let event = unsafe { AutoResetEvent::embedded(container.as_ref()) };

            {
                let _future = event.wait();
            }
            event.set();
            event.wait().await;
        });
    }

    // --- manual-poll tests (cover register→wake→ready cycle) ---

    #[test]
    fn notified_then_dropped_re_sets_event() {
        let event = AutoResetEvent::boxed();
        let mut future = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        // Poll to register.
        assert!(future.as_mut().poll(&mut cx).is_pending());

        // set() pops the waiter and marks it notified.
        event.set();

        // Drop the notified future without re-polling. No other waiters
        // exist, so Drop must re-set the event.
        drop(future);

        assert!(event.try_wait());
    }

    #[test]
    fn notified_then_dropped_while_set_preserves_signal() {
        let event = AutoResetEvent::boxed();
        let mut future = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        // Poll to register.
        assert!(future.as_mut().poll(&mut cx).is_pending());

        // First set() pops the waiter and marks it notified.
        event.set();

        // Second set() transitions the event back to Set (no
        // waiters remain in the set).
        event.set();

        // Drop the notified future. The state is already Set, so
        // drop_wait must preserve the signal.
        drop(future);

        assert!(event.try_wait());
    }

    #[test]
    fn notified_then_dropped_forwards_to_next() {
        let event = AutoResetEvent::boxed();
        let mut future1 = Box::pin(event.wait());
        let mut future2 = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        // Both register.
        assert!(future1.as_mut().poll(&mut cx).is_pending());
        assert!(future2.as_mut().poll(&mut cx).is_pending());

        // set() notifies the first registered future.
        event.set();

        // Drop future1 without re-polling — notification should forward
        // to future2.
        drop(future1);

        // future2 should now be notified.
        assert!(future2.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn set_wakes_registered_waiter() {
        use crate::test_helpers::AtomicWakeTracker;

        let event = AutoResetEvent::boxed();

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
    fn embedded_wait_registers_then_completes() {
        let container = Box::pin(EmbeddedAutoResetEvent::new());
        // SAFETY: The container outlives the handle within this test.
        let event = unsafe { AutoResetEvent::embedded(container.as_ref()) };

        let mut future = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        // First poll — not set, registers in awaiter set.
        assert!(future.as_mut().poll(&mut cx).is_pending());

        // set() pops and notifies the waiter.
        event.set();

        // Second poll — sees notified flag, returns Ready.
        assert!(future.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn embedded_drop_registered_future() {
        let container = Box::pin(EmbeddedAutoResetEvent::new());
        // SAFETY: The container outlives the handle within this test.
        let event = unsafe { AutoResetEvent::embedded(container.as_ref()) };

        let mut future = Box::pin(event.wait());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(future.as_mut().poll(&mut cx).is_pending());

        // Drop a registered (not notified) future.
        drop(future);

        // Event should still work.
        event.set();
        assert!(event.try_wait());
    }

    #[test]
    fn embedded_notified_then_dropped_re_sets_event() {
        let container = Box::pin(EmbeddedAutoResetEvent::new());
        // SAFETY: The container outlives the handle within this test.
        let event = unsafe { AutoResetEvent::embedded(container.as_ref()) };

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
    fn embedded_notified_then_dropped_forwards_to_next() {
        let container = Box::pin(EmbeddedAutoResetEvent::new());
        // SAFETY: The container outlives the handle within this test.
        let event = unsafe { AutoResetEvent::embedded(container.as_ref()) };

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
    fn embedded_set_wakes_registered_waiter() {
        use crate::test_helpers::AtomicWakeTracker;

        let container = Box::pin(EmbeddedAutoResetEvent::new());
        // SAFETY: The container outlives the handle.
        let event = unsafe { AutoResetEvent::embedded(container.as_ref()) };

        let tracker = AtomicWakeTracker::new();
        // SAFETY: The tracker outlives the waker.
        let waker = unsafe { tracker.waker() };
        let mut cx = task::Context::from_waker(&waker);

        let mut future = Box::pin(event.wait());
        assert!(future.as_mut().poll(&mut cx).is_pending());

        event.set();

        assert!(tracker.was_woken());
    }

    // This tests a defense-in-depth branch in poll() that unregisters
    // a waiter when is_set is observed while still registered. In normal
    // usage, set() pops a waiter rather than setting is_set when the set
    // is non-empty, so this state cannot arise through the public API.
    // We force it by directly manipulating the guarded state.
    //
    // NOTE: These tests were removed because the enum-based State type
    // makes the "is_set + waiters" combination structurally impossible.

    const WAITER_COUNT: usize = 100;

    #[test]
    fn many_sets_release_all_waiters() {
        let event = AutoResetEvent::boxed();
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
        let container = Box::pin(EmbeddedAutoResetEvent::new());
        // SAFETY: The container outlives the handle within this test.
        let event = unsafe { AutoResetEvent::embedded(container.as_ref()) };
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
        let event = AutoResetEvent::boxed();

        for _ in 0..WAITER_COUNT {
            event.set();
        }

        // Only one signal should be latched.
        assert!(event.try_wait());
        assert!(!event.try_wait());
    }

    #[test]
    fn set_with_reentrant_waker_does_not_deadlock() {
        use testing::ReentrantWakerData;

        let event = AutoResetEvent::boxed();
        let event_for_waker = event.clone();

        let waker_data = ReentrantWakerData::new(move || {
            // Re-entrantly call set() on the same event.
            event_for_waker.set();
        });
        // SAFETY: Data outlives waker, test is single-threaded.
        let waker = unsafe { waker_data.waker() };
        let mut cx = task::Context::from_waker(&waker);

        let mut future = Box::pin(event.wait());
        assert!(future.as_mut().poll(&mut cx).is_pending());

        // set() notifies the future, calling the re-entrant waker
        // which calls set() again. The second set() should store
        // the signal (no waiters left).
        event.set();

        assert!(waker_data.was_woken());
        // The re-entrant set() stored a signal.
        assert!(event.try_wait());
    }

    #[test]
    fn embedded_default_creates_unset_event() {
        let container = Box::pin(EmbeddedAutoResetEvent::default());
        // SAFETY: The container outlives the handle.
        let event = unsafe { AutoResetEvent::embedded(container.as_ref()) };
        assert!(!event.try_wait());
    }
}
