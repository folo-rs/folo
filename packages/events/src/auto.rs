use std::cell::UnsafeCell;
use std::fmt;
use std::future::Future;
use std::marker::PhantomPinned;
use std::mem;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::task::{self, Poll, Waker};

use crate::NEVER_POISONED;
use waiter_list::{WaiterList, WaiterNode};

/// Thread-safe async auto-reset event.
///
/// Releases exactly one awaiter per [`set()`][Self::set] call.
///
/// If no one is waiting when `set()` is called, the event remembers the signal
/// so that the next [`wait()`][Self::wait] completes immediately (consuming the
/// signal). If one or more tasks are waiting, a single waiter is released and
/// the event stays unset.
///
/// The event is a lightweight cloneable handle. All clones derived from the
/// same [`boxed()`][Self::boxed] call share the same underlying state.
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
    state: Arc<Mutex<State>>,
}

// The signal flag and waiter list are mutually exclusive: if the event is set
// there are no waiters, and if there are waiters the event is not set. This
// enum encodes that invariant at the type level.
enum State {
    /// Not signaled. The waiter list may be empty or non-empty.
    Unset(WaiterList),
    /// Signal stored (will be consumed by the next wait or `try_wait`).
    Set,
}

// Marker trait impl.
// SAFETY: The raw pointers inside WaiterList are only dereferenced while the
// Mutex is held, ensuring exclusive access.
unsafe impl Send for State {}

// Mutating set() to a no-op causes wait futures to hang.
#[cfg_attr(test, mutants::skip)]
fn set(mutex: &Mutex<State>) {
    let waker: Option<Waker>;

    {
        let mut state = mutex.lock().expect(NEVER_POISONED);

        match &mut *state {
            State::Set => {
                waker = None;
            }
            State::Unset(waiters) => {
                // SAFETY: We hold the lock.
                if let Some(node_ptr) = unsafe { waiters.pop_front() } {
                    // SAFETY: We hold the lock and just popped this
                    // node.
                    unsafe {
                        (*node_ptr).set_notified();
                    }

                    // SAFETY: Same node, we hold the lock.
                    waker = unsafe { (*node_ptr).take_waker() };
                } else {
                    // No waiters — store the signal.
                    *state = State::Set;
                    waker = None;
                }
            }
        }
    }

    if let Some(w) = waker {
        w.wake();
    }
}

// Mutating try_wait() to return false causes spin-loop tests to hang.
#[cfg_attr(test, mutants::skip)]
fn try_wait(mutex: &Mutex<State>) -> bool {
    let mut state = mutex.lock().expect(NEVER_POISONED);
    if matches!(*state, State::Set) {
        *state = State::Unset(WaiterList::new());
        true
    } else {
        false
    }
}

/// Shared poll logic for both `AutoResetWaitFuture` and
/// `RawAutoResetWaitFuture`.
///
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
    let node_ptr = node.get();

    let mut state = mutex.lock().expect(NEVER_POISONED);

    // Check if we were directly notified by set() (it popped us
    // from the list and set our notified flag).
    // SAFETY: We hold the lock.
    if unsafe { (*node_ptr).is_notified() } {
        *registered = false;
        return Poll::Ready(());
    }

    match &mut *state {
        State::Set => {
            // Signal available — consume it.
            debug_assert!(
                !*registered,
                "Set state is exclusive with registered waiters"
            );
            *state = State::Unset(WaiterList::new());
            Poll::Ready(())
        }
        State::Unset(waiters) => {
            // SAFETY: We hold the lock.
            unsafe {
                (*node_ptr).store_waker(waker);
            }
            if !*registered {
                // SAFETY: We hold the lock, node is pinned and not
                // in any list.
                unsafe {
                    waiters.push_back(node_ptr);
                }
                *registered = true;
            }
            Poll::Pending
        }
    }
}

/// Shared drop logic for both wait future types.
///
/// # Safety
///
/// Same requirements as [`poll_wait`].
unsafe fn drop_wait(mutex: &Mutex<State>, node: &UnsafeCell<WaiterNode>, registered: bool) {
    // The caller must only call this when the node is registered. Both
    // AutoResetWaitFuture::drop and RawAutoResetWaitFuture::drop guard
    // on `self.registered` before calling, so this should always hold.
    debug_assert!(registered);

    let node_ptr = node.get();
    let mut state = mutex.lock().expect(NEVER_POISONED);

    // SAFETY: We hold the lock.
    if unsafe { (*node_ptr).is_notified() } {
        // We were notified but the future was cancelled before it
        // could complete. Forward the notification to the next
        // waiter so that no signal is lost.
        let old_state = mem::replace(&mut *state, State::Unset(WaiterList::new()));
        match old_state {
            State::Unset(mut waiters) => {
                // SAFETY: We hold the lock.
                if let Some(next_node) = unsafe { waiters.pop_front() } {
                    // SAFETY: We hold the lock and just popped
                    // this node.
                    unsafe {
                        (*next_node).set_notified();
                    }
                    // SAFETY: Same node, we hold the lock.
                    let waker = unsafe { (*next_node).take_waker() };
                    // Restore the waiter list.
                    *state = State::Unset(waiters);
                    drop(state);

                    if let Some(w) = waker {
                        w.wake();
                    }
                } else {
                    // No more waiters — restore the signal so it
                    // is not lost.
                    *state = State::Set;
                }
            }
            State::Set => {
                // Already set — restore.
                *state = State::Set;
            }
        }
    } else {
        // Not notified — just remove from the list.
        match &mut *state {
            State::Unset(waiters) => {
                // SAFETY: We hold the lock and the node is in the
                // list.
                unsafe {
                    waiters.remove(node_ptr);
                }
            }
            State::Set => {
                // Not notified + registered ⟹ node is in a waiter
                // list ⟹ state must be Unset.
                debug_assert!(false, "registered non-notified node requires Unset state");
            }
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
            state: Arc::new(Mutex::new(State::Unset(WaiterList::new()))),
        }
    }

    /// Creates a handle from an [`EmbeddedAutoResetEvent`] container,
    /// avoiding heap allocation.
    ///
    /// Calling this multiple times on the same container is safe and
    /// returns handles that all operate on the same shared state, just
    /// like copying or cloning a [`RawAutoResetEvent`].
    ///
    /// # Safety
    ///
    /// The caller must ensure that the [`EmbeddedAutoResetEvent`] outlives
    /// all returned handles and any [`RawAutoResetWaitFuture`]s created
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
    pub unsafe fn embedded(place: Pin<&EmbeddedAutoResetEvent>) -> RawAutoResetEvent {
        let state = NonNull::from(&place.get_ref().state);
        RawAutoResetEvent { state }
    }

    /// Signals the event, releasing exactly one waiter.
    ///
    /// * If one or more tasks are waiting, a single waiter is released and
    ///   the event remains unset.
    /// * If no task is waiting, the event transitions to the set state so that
    ///   the next [`wait()`][Self::wait] or [`try_wait()`][Self::try_wait]
    ///   completes immediately.
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
    // Mutating set() to a no-op causes wait futures to hang.
    #[cfg_attr(test, mutants::skip)]
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn set(&self) {
        set(&self.state);
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
    // Mutating try_wait() to return false causes spin-loop tests to hang.
    #[cfg_attr(test, mutants::skip)]
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn try_wait(&self) -> bool {
        try_wait(&self.state)
    }

    /// Returns a future that completes when the event is signaled.
    ///
    /// When [`set()`][Self::set] is called, a single waiting future is
    /// released. If the event is already set (no prior waiter consumed it),
    /// the future completes immediately and consumes the signal.
    ///
    /// # Cancellation safety
    ///
    /// If a future that has been notified is dropped before it is polled to
    /// completion, the notification is forwarded to the next waiter (or the
    /// event is re-set if no waiters remain). No signals are lost due to
    /// cancellation.
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
            state: Arc::clone(&self.state),
            node: UnsafeCell::new(WaiterNode::new()),
            registered: false,
            _pinned: PhantomPinned,
        }
    }
}

/// Future returned by [`AutoResetEvent::wait()`].
///
/// Completes with `()` when the event signal is acquired.
pub struct AutoResetWaitFuture {
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
// WaiterNode are only dereferenced under the Mutex.
unsafe impl Send for AutoResetWaitFuture {}

// The UnsafeCell<WaiterNode> field causes auto-trait inference to mark the
// future as !UnwindSafe and !RefUnwindSafe. However, all mutable access to
// the node goes through the Mutex, preventing inconsistent state observation.
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

        // SAFETY: The node is pinned (PhantomPinned) and the state
        // field is the mutex this node registers with.
        unsafe { poll_wait(&this.state, &this.node, &mut this.registered, &waker) }
    }
}

impl Drop for AutoResetWaitFuture {
    fn drop(&mut self) {
        if !self.registered {
            return;
        }

        // SAFETY: The node is pinned (PhantomPinned) and the state
        // field is the mutex this node was registered with.
        unsafe { drop_wait(&self.state, &self.node, self.registered) }
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
            .field("registered", &self.registered)
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
/// to obtain a [`RawAutoResetEvent`] handle.
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
    state: Mutex<State>,
    _pinned: PhantomPinned,
}

impl EmbeddedAutoResetEvent {
    /// Creates a new embedded event container in the unset state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State::Unset(WaiterList::new())),
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
pub struct RawAutoResetEvent {
    state: NonNull<Mutex<State>>,
}

// Marker trait impl.
// SAFETY: Mutex<State> is Send + Sync. The raw pointer is only dereferenced
// to obtain &Mutex<State>, which is safe to share across threads.
unsafe impl Send for RawAutoResetEvent {}

// Marker trait impl.
// SAFETY: Same as Send — all mutable access is mediated by the Mutex.
unsafe impl Sync for RawAutoResetEvent {}

// Marker trait impl.
impl UnwindSafe for RawAutoResetEvent {}
// Marker trait impl.
impl RefUnwindSafe for RawAutoResetEvent {}

impl RawAutoResetEvent {
    fn state(&self) -> &Mutex<State> {
        // SAFETY: The caller of `embedded()` guarantees the container
        // outlives this handle.
        unsafe { self.state.as_ref() }
    }

    /// Signals the event, releasing exactly one waiter.
    // Mutating set() to a no-op causes wait futures to hang.
    #[cfg_attr(test, mutants::skip)]
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn set(&self) {
        set(self.state());
    }

    /// Attempts to consume the signal without blocking.
    ///
    /// Returns `true` if the event was set, atomically transitioning it
    /// back to the unset state. Returns `false` if the event was not set.
    #[must_use]
    // Mutating try_wait() to return false causes spin-loop tests to hang.
    #[cfg_attr(test, mutants::skip)]
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn try_wait(&self) -> bool {
        try_wait(self.state())
    }

    /// Returns a future that completes when the event is signaled.
    #[must_use]
    pub fn wait(&self) -> RawAutoResetWaitFuture {
        RawAutoResetWaitFuture {
            state: self.state,
            node: UnsafeCell::new(WaiterNode::new()),
            registered: false,
            _pinned: PhantomPinned,
        }
    }
}

/// Future returned by [`RawAutoResetEvent::wait()`].
pub struct RawAutoResetWaitFuture {
    state: NonNull<Mutex<State>>,

    // See AutoResetWaitFuture for field documentation.
    node: UnsafeCell<WaiterNode>,
    registered: bool,

    _pinned: PhantomPinned,
}

// Marker trait impl.
// SAFETY: Same reasoning as AutoResetWaitFuture — all node access is
// protected by the Mutex.
unsafe impl Send for RawAutoResetWaitFuture {}

// Marker trait impl.
impl UnwindSafe for RawAutoResetWaitFuture {}
// Marker trait impl.
impl RefUnwindSafe for RawAutoResetWaitFuture {}

impl Future for RawAutoResetWaitFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<()> {
        // Clone the waker before acquiring the lock so a panicking clone
        // implementation cannot poison the mutex.
        let waker = cx.waker().clone();

        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };

        // SAFETY: The container outlives this future per the embedded()
        // contract.
        let state = unsafe { this.state.as_ref() };
        // SAFETY: The node is pinned (PhantomPinned) and the state
        // is the mutex this node registers with.
        unsafe { poll_wait(state, &this.node, &mut this.registered, &waker) }
    }
}

impl Drop for RawAutoResetWaitFuture {
    fn drop(&mut self) {
        if !self.registered {
            return;
        }

        // SAFETY: The container outlives this future per the embedded()
        // contract.
        let state = unsafe { self.state.as_ref() };
        // SAFETY: The node is pinned (PhantomPinned) and the state
        // is the mutex this node was registered with.
        unsafe { drop_wait(state, &self.node, self.registered) }
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
impl fmt::Debug for RawAutoResetEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawAutoResetEvent").finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for RawAutoResetWaitFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawAutoResetWaitFuture")
            .field("registered", &self.registered)
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

    // --- trait assertions ---

    assert_impl_all!(AutoResetEvent: Send, Sync, Clone, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(AutoResetWaitFuture: Send, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(AutoResetWaitFuture: Sync, Unpin);

    assert_impl_all!(EmbeddedAutoResetEvent: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(EmbeddedAutoResetEvent: Unpin);
    assert_impl_all!(RawAutoResetEvent: Send, Sync, Clone, Copy, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RawAutoResetWaitFuture: Send, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(RawAutoResetWaitFuture: Sync, Unpin);

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
        assert!(f1.as_mut().poll(&mut cx).is_ready());
        assert!(f2.as_mut().poll(&mut cx).is_pending());
        assert!(f3.as_mut().poll(&mut cx).is_pending());

        // Signal twice more to release the remaining two.
        event.set();
        assert!(f2.as_mut().poll(&mut cx).is_ready());

        event.set();
        assert!(f3.as_mut().poll(&mut cx).is_ready());
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

        // First poll — not set, registers in waiter list.
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
    // usage, set() pops a waiter rather than setting is_set when the list
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

        for f in &mut futures {
            event.set();
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
}
