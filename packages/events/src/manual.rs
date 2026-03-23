use std::cell::UnsafeCell;
use std::fmt;
use std::future::Future;
use std::marker::PhantomPinned;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::task::{self, Poll, Waker};

use crate::NEVER_POISONED;
use waiter_list::{WaiterList, WaiterNode};

/// Thread-safe async manual-reset event.
///
/// Once set, releases all current and future awaiters until explicitly reset.
///
/// A `ManualResetEvent` acts as a gate: while set, every call to
/// [`wait()`][Self::wait] completes immediately. Calling [`reset()`][Self::reset]
/// closes the gate so that subsequent awaiters block until the next
/// [`set()`][Self::set].
///
/// The event is a lightweight cloneable handle. All clones derived from the
/// same [`boxed()`][Self::boxed] call share the same underlying state.
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
    state: Arc<Mutex<State>>,
}

struct State {
    /// Whether the event is currently in the signaled state. Unlike
    /// auto-reset events, this is not mutually exclusive with waiters —
    /// waiters stay registered while the event is set and are woken
    /// one-by-one in a loop.
    is_set: bool,
    waiters: WaiterList,
}

// Marker trait impl.
// SAFETY: The raw pointers inside WaiterList are only dereferenced while the
// Mutex is held, ensuring exclusive access.
unsafe impl Send for State {}

// Test hook that fires after each wake() call in set(). This allows tests to
// inject operations (e.g. calling reset() and re-polling a future) between
// the wake and the re-acquisition of the lock, reproducing race conditions
// that would otherwise require precise multi-thread timing.
#[cfg(test)]
type SetHookFn = dyn Fn() + Send + Sync;

#[cfg(test)]
static HOOK_SET_AFTER_WAKE: Mutex<Option<Arc<SetHookFn>>> = Mutex::new(None);

// Mutating set() to a no-op causes wait futures to hang.
#[cfg_attr(test, mutants::skip)]
fn set(mutex: &Mutex<State>) {
    let mut state = mutex.lock().expect(NEVER_POISONED);
    if state.is_set {
        return;
    }
    state.is_set = true;

    // Wake all waiters using the rescan-from-head pattern.
    // We drop the lock before waking each waiter to prevent deadlocks
    // from re-entrant wakers. After waking, we re-acquire the lock and
    // start scanning from the head again because nodes may have been
    // removed during the unlock window.
    loop {
        let waker = {
            let mut cursor = state.waiters.head();
            loop {
                if cursor.is_null() {
                    break None;
                }
                // SAFETY: We hold the lock.
                let w = unsafe { (*cursor).take_waker() };
                if w.is_some() {
                    break w;
                }
                // SAFETY: We hold the lock.
                cursor = unsafe { (*cursor).next_in_list() };
            }
        };

        let Some(w) = waker else { break };
        drop(state);
        w.wake();

        #[cfg(test)]
        {
            let hook = HOOK_SET_AFTER_WAKE.lock().expect(NEVER_POISONED).clone();
            if let Some(hook) = hook {
                hook();
            }
        }

        state = mutex.lock().expect(NEVER_POISONED);

        // If someone called reset() while the lock was released, stop
        // waking — the gate has been closed.
        if !state.is_set {
            break;
        }
    }
}

fn reset(mutex: &Mutex<State>) {
    let mut state = mutex.lock().expect(NEVER_POISONED);
    state.is_set = false;
}

// Mutating try_wait() to return false causes spin-loop tests to hang.
#[cfg_attr(test, mutants::skip)]
fn try_wait(mutex: &Mutex<State>) -> bool {
    let state = mutex.lock().expect(NEVER_POISONED);
    state.is_set
}

/// # Safety
///
/// * The `node` must be pinned and must remain at the same memory address
///   for the lifetime of the wait future.
/// * The `mutex` must protect the waiter list that this node is (or will
///   be) registered with.
unsafe fn poll_wait(
    mutex: &Mutex<State>,
    node: &UnsafeCell<WaiterNode>,
    registered: &mut bool,
    waker: &Waker,
) -> Poll<()> {
    let mut state = mutex.lock().expect(NEVER_POISONED);
    let node_ptr = node.get();

    if state.is_set {
        if *registered {
            // SAFETY: We hold the lock and the node is in the list.
            unsafe {
                state.waiters.remove(node_ptr);
            }
            *registered = false;
        }
        return Poll::Ready(());
    }

    // SAFETY: We hold the lock.
    unsafe {
        (*node_ptr).store_waker(waker);
    }

    if !*registered {
        // SAFETY: We hold the lock, node is pinned and not in any list.
        unsafe {
            state.waiters.push_back(node_ptr);
        }
        *registered = true;
    }

    Poll::Pending
}

/// # Safety
///
/// Same requirements as [`poll_wait`].
unsafe fn drop_wait(mutex: &Mutex<State>, node: &UnsafeCell<WaiterNode>, registered: bool) {
    if registered {
        let mut state = mutex.lock().expect(NEVER_POISONED);
        // SAFETY: We hold the lock and the node is in the list.
        unsafe {
            state.waiters.remove(node.get());
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
            state: Arc::new(Mutex::new(State {
                is_set: false,
                waiters: WaiterList::new(),
            })),
        }
    }

    /// Creates a handle from an [`EmbeddedManualResetEvent`] container,
    /// avoiding heap allocation.
    ///
    /// Calling this multiple times on the same container is safe and
    /// returns handles that all operate on the same shared state, just
    /// like copying or cloning a [`RawManualResetEvent`].
    ///
    /// # Safety
    ///
    /// The caller must ensure that the [`EmbeddedManualResetEvent`] outlives
    /// all returned handles and any [`RawManualResetWaitFuture`]s created
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
    pub unsafe fn embedded(place: Pin<&EmbeddedManualResetEvent>) -> RawManualResetEvent {
        let state = NonNull::from(&place.get_ref().state);
        RawManualResetEvent { state }
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
    // Mutating set() to a no-op causes wait futures to hang. We cannot
    // detect "wait never completes" without real-time timeouts.
    #[cfg_attr(test, mutants::skip)]
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn set(&self) {
        set(&self.state);
    }

    /// Closes the gate. Future calls to [`wait()`][Self::wait] will block
    /// until the next [`set()`][Self::set].
    ///
    /// Awaiters that are already past the gate (i.e. whose futures have
    /// already returned [`Poll::Ready`]) are not affected.
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn reset(&self) {
        reset(&self.state);
    }

    /// Returns `true` if the event is currently set.
    ///
    /// Because other threads may set or reset the event concurrently, the
    /// returned value is immediately stale. Use this for diagnostics or
    /// best-effort checks, not for synchronization.
    #[must_use]
    // Mutating try_wait() to return false causes spin-loop tests to hang.
    #[cfg_attr(test, mutants::skip)]
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn try_wait(&self) -> bool {
        try_wait(&self.state)
    }

    /// Returns a future that completes when the event is set.
    ///
    /// If the event is already set at the time of polling, the future
    /// completes immediately. If the event is reset between being woken and
    /// being re-polled, the future goes back to pending.
    ///
    /// The returned future is `Send` and can be passed to other tasks
    /// freely.
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
            state: Arc::clone(&self.state),
            node: UnsafeCell::new(WaiterNode::new()),
            registered: false,
            _pinned: PhantomPinned,
        }
    }
}

/// Future returned by [`ManualResetEvent::wait()`].
///
/// Completes with `()` when the event is in the set state at the time of
/// polling.
pub struct ManualResetWaitFuture {
    state: Arc<Mutex<State>>,

    // Behind UnsafeCell so that raw pointers from the event's waiter list can
    // coexist with the &mut Self we obtain in poll() via get_unchecked_mut().
    // UnsafeCell opts out of the noalias guarantee for its contents.
    node: UnsafeCell<WaiterNode>,

    // Whether this future's node is currently in the event's waiter list.
    // Only accessed through &mut Self in poll()/drop(), never through the list.
    registered: bool,

    _pinned: PhantomPinned,
}

// Marker trait impl.
// SAFETY: All UnsafeCell<WaiterNode> fields are accessed exclusively under the
// event's Mutex. The Arc<Mutex<State>> is Send + Sync. The raw pointers inside
// WaiterNode point to nodes in other futures that may be on other threads, but
// are only dereferenced under the Mutex.
unsafe impl Send for ManualResetWaitFuture {}

// The UnsafeCell<WaiterNode> field causes auto-trait inference to mark the
// future as !UnwindSafe and !RefUnwindSafe. However, a shared reference cannot
// observe inconsistent state because all mutable access to the node goes
// through the Mutex or through Pin<&mut Self> (which is exclusive).
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
        // SAFETY: The node is pinned (PhantomPinned) and the state
        // field is the mutex this node registers with.
        unsafe { poll_wait(&this.state, &this.node, &mut this.registered, &waker) }
    }
}

impl Drop for ManualResetWaitFuture {
    fn drop(&mut self) {
        // SAFETY: The node is pinned (PhantomPinned) and the state
        // field is the mutex this node was registered with.
        unsafe { drop_wait(&self.state, &self.node, self.registered) }
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
            .field("registered", &self.registered)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Embedded variant
// ---------------------------------------------------------------------------

/// Embedded-state container for [`ManualResetEvent`].
///
/// Stores the event state inline in a struct, avoiding the heap allocation
/// that [`ManualResetEvent::boxed()`] requires. Create the container with
/// [`new()`][Self::new], pin it, then call [`ManualResetEvent::embedded()`]
/// to obtain a [`RawManualResetEvent`] handle.
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
    state: Mutex<State>,
    _pinned: PhantomPinned,
}

impl EmbeddedManualResetEvent {
    /// Creates a new embedded event container in the unset state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State {
                is_set: false,
                waiters: WaiterList::new(),
            }),
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
pub struct RawManualResetEvent {
    state: NonNull<Mutex<State>>,
}

// Marker trait impl.
// SAFETY: Mutex<State> is Send + Sync. The raw pointer is only dereferenced to
// obtain &Mutex<State>, which is safe to share across threads.
unsafe impl Send for RawManualResetEvent {}

// Marker trait impl.
// SAFETY: Same as Send — all mutable access is mediated by the Mutex.
unsafe impl Sync for RawManualResetEvent {}

// Marker trait impl.
impl UnwindSafe for RawManualResetEvent {}
// Marker trait impl.
impl RefUnwindSafe for RawManualResetEvent {}

impl RawManualResetEvent {
    fn state(&self) -> &Mutex<State> {
        // SAFETY: The caller of `embedded()` guarantees the container
        // outlives this handle.
        unsafe { self.state.as_ref() }
    }

    /// Opens the gate, releasing all current awaiters.
    ///
    /// If the event is already set, this is a no-op.
    // Mutating set() to a no-op causes wait futures to hang.
    #[cfg_attr(test, mutants::skip)]
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn set(&self) {
        set(self.state());
    }

    /// Closes the gate.
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn reset(&self) {
        reset(self.state());
    }

    /// Returns `true` if the event is currently set.
    #[must_use]
    // Mutating try_wait() to return false causes spin-loop tests to hang.
    #[cfg_attr(test, mutants::skip)]
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn try_wait(&self) -> bool {
        try_wait(self.state())
    }

    /// Returns a future that completes when the event is set.
    #[must_use]
    pub fn wait(&self) -> RawManualResetWaitFuture {
        RawManualResetWaitFuture {
            state: self.state,
            node: UnsafeCell::new(WaiterNode::new()),
            registered: false,
            _pinned: PhantomPinned,
        }
    }
}

/// Future returned by [`RawManualResetEvent::wait()`].
pub struct RawManualResetWaitFuture {
    state: NonNull<Mutex<State>>,

    // See ManualResetWaitFuture for field documentation.
    node: UnsafeCell<WaiterNode>,
    registered: bool,

    _pinned: PhantomPinned,
}

// Marker trait impl.
// SAFETY: Same reasoning as ManualResetWaitFuture — all node access is
// protected by the Mutex.
unsafe impl Send for RawManualResetWaitFuture {}

// Marker trait impl.
impl UnwindSafe for RawManualResetWaitFuture {}
// Marker trait impl.
impl RefUnwindSafe for RawManualResetWaitFuture {}

impl Future for RawManualResetWaitFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<()> {
        let waker = cx.waker().clone();
        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };
        // SAFETY: The container outlives this future. Node is pinned via
        // PhantomPinned.
        let state = unsafe { this.state.as_ref() };
        // SAFETY: The node is pinned (PhantomPinned) and the state
        // is the mutex this node registers with.
        unsafe { poll_wait(state, &this.node, &mut this.registered, &waker) }
    }
}

impl Drop for RawManualResetWaitFuture {
    fn drop(&mut self) {
        // SAFETY: The container outlives this future. Node is pinned via
        // PhantomPinned.
        let state = unsafe { self.state.as_ref() };
        // SAFETY: The node is pinned (PhantomPinned) and the state
        // is the mutex this node was registered with.
        unsafe { drop_wait(state, &self.node, self.registered) }
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
impl fmt::Debug for RawManualResetEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let is_set = self.try_wait();
        f.debug_struct("RawManualResetEvent")
            .field("is_set", &is_set)
            .finish()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for RawManualResetWaitFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawManualResetWaitFuture")
            .field("registered", &self.registered)
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

    // --- trait assertions ---

    assert_impl_all!(ManualResetEvent: Send, Sync, Clone, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(ManualResetWaitFuture: Send, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(ManualResetWaitFuture: Sync, Unpin);

    assert_impl_all!(EmbeddedManualResetEvent: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(EmbeddedManualResetEvent: Unpin);
    assert_impl_all!(RawManualResetEvent: Send, Sync, Clone, Copy, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RawManualResetWaitFuture: Send, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(RawManualResetWaitFuture: Sync, Unpin);

    // --- basic functionality ---

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

    // --- async tests ---

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

        // Poll once to register in the waiter list.
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

    // --- multithreaded tests (Miri-compatible) ---

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

    // --- embedded variant tests ---

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

        // First poll — not set, registers in waiter list.
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

        // Drop future1 — its node must be removed from the list.
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

    // --- livelock regression test ---

    /// Verifies that `set()` terminates even if `reset()` is called while
    /// the wake loop is in progress.
    ///
    /// Without the `is_set` re-check after re-acquiring the lock, `set()`
    /// would loop forever: it wakes a waiter, the hook resets the event
    /// and re-polls the future (re-storing a waker), and `set()` rescans
    /// from head — finding the fresh waker and repeating indefinitely.
    #[test]
    fn set_terminates_when_reset_called_during_wake_loop() {
        testing::with_watchdog(|| {
            let event = ManualResetEvent::boxed();
            let mut future = Box::pin(event.wait());

            // First poll: register as a waiter with a noop waker.
            let waker = Waker::noop();
            let mut cx = task::Context::from_waker(waker);
            assert!(future.as_mut().poll(&mut cx).is_pending());

            // Install hook: after each wake(), reset the event and re-poll
            // the future so it re-stores a waker. This simulates a scenario
            // where another thread calls reset() and the executor re-polls
            // the woken future between set()'s lock drops.
            let event_for_hook = event.clone();
            let future_shared: Arc<Mutex<Pin<Box<ManualResetWaitFuture>>>> =
                Arc::new(Mutex::new(future));
            let future_for_hook = Arc::clone(&future_shared);

            *HOOK_SET_AFTER_WAKE.lock().unwrap() = Some(Arc::new(move || {
                event_for_hook.reset();
                let w = Waker::noop();
                let mut cx = task::Context::from_waker(w);
                let _poll = future_for_hook.lock().unwrap().as_mut().poll(&mut cx);
            }));

            event.set();

            // Cleanup.
            *HOOK_SET_AFTER_WAKE.lock().unwrap() = None;
        });
    }
}
