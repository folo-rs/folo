use std::cell::UnsafeCell;
use std::fmt;
use std::future::Future;
use std::marker::PhantomPinned;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::task::{self, Poll, Waker};

use crate::waiter_list::{WaiterList, WaiterNode};

const NEVER_POISONED: &str = "we never panic while holding this lock";

/// Thread-safe async event that releases exactly one awaiter per
/// [`set()`][Self::set] call.
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
///     assert!(!event.try_acquire());
/// }
/// ```
#[derive(Clone)]
pub struct AutoResetEvent {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<GuardedState>,
}

struct GuardedState {
    is_set: bool,
    waiters: WaiterList,
}

#[cfg_attr(coverage_nightly, coverage(off))] // Marker trait impl.
// SAFETY: The raw pointers inside WaiterList are only dereferenced while the
// Mutex is held, ensuring exclusive access.
unsafe impl Send for GuardedState {}

impl AutoResetEvent {
    /// Creates a new event in the unset state.
    ///
    /// The returned handle is backed by a heap-allocated shared state
    /// ([`Arc`]). Clone the handle to obtain additional references to the
    /// same event.
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
    /// assert!(event.try_acquire());
    /// ```
    #[must_use]
    pub fn boxed() -> Self {
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(GuardedState {
                    is_set: false,
                    waiters: WaiterList::new(),
                }),
            }),
        }
    }

    /// Creates a handle backed by an [`EmbeddedAutoResetEvent`] container
    /// instead of a heap-allocated [`Arc`].
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
    /// // SAFETY: The container outlives the handle.
    /// let event = unsafe { AutoResetEvent::embedded(container.as_ref()) };
    /// let setter = event;
    ///
    /// setter.set();
    /// event.wait().await;
    /// # });
    /// ```
    #[must_use]
    pub unsafe fn embedded(place: Pin<&EmbeddedAutoResetEvent>) -> RawAutoResetEvent {
        let inner = NonNull::from(&place.get_ref().inner);
        RawAutoResetEvent { inner }
    }

    /// Signals the event, releasing exactly one waiter.
    ///
    /// * If one or more tasks are waiting, a single waiter is released and
    ///   the event remains unset.
    /// * If no task is waiting, the event transitions to the set state so that
    ///   the next [`wait()`][Self::wait] or [`try_acquire()`][Self::try_acquire]
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
    pub fn set(&self) {
        let waker: Option<Waker>;

        {
            let mut state = self.inner.state.lock().expect(NEVER_POISONED);

            // SAFETY: We hold the lock, so all node pointers are valid.
            if let Some(node_ptr) = unsafe { state.waiters.pop_front() } {
                // Notify the next waiter.
                // SAFETY: We hold the lock and just popped this node.
                unsafe {
                    (*node_ptr).notified = true;
                }

                // SAFETY: Same node, we hold the lock.
                waker = unsafe { (*node_ptr).waker.take() };
            } else {
                // No waiters — store the signal for the next waiter.
                state.is_set = true;
                waker = None;
            }
        }

        if let Some(w) = waker {
            w.wake();
        }
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
    /// assert!(!event.try_acquire());
    ///
    /// event.set();
    /// assert!(event.try_acquire());
    ///
    /// // Signal was consumed.
    /// assert!(!event.try_acquire());
    /// ```
    #[must_use]
    // Mutating try_acquire() to return false causes spin-loop tests to hang.
    #[cfg_attr(test, mutants::skip)]
    pub fn try_acquire(&self) -> bool {
        let mut state = self.inner.state.lock().expect(NEVER_POISONED);
        if state.is_set {
            state.is_set = false;
            true
        } else {
            false
        }
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
            inner: Arc::clone(&self.inner),
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
    inner: Arc<Inner>,
    node: UnsafeCell<WaiterNode>,
    registered: bool,
    _pinned: PhantomPinned,
}

#[cfg_attr(coverage_nightly, coverage(off))] // Marker trait impl.
// SAFETY: All UnsafeCell<WaiterNode> fields are accessed exclusively under the
// event's Mutex. The Arc<Inner> is Send + Sync. The raw pointers inside
// WaiterNode are only dereferenced under the Mutex.
unsafe impl Send for AutoResetWaitFuture {}

// The UnsafeCell<WaiterNode> field causes auto-trait inference to mark the
// future as !UnwindSafe and !RefUnwindSafe. However, all mutable access to
// the node goes through the Mutex, preventing inconsistent state observation.
#[cfg_attr(coverage_nightly, coverage(off))] // Marker trait impl.
impl UnwindSafe for AutoResetWaitFuture {}
#[cfg_attr(coverage_nightly, coverage(off))] // Marker trait impl.
impl RefUnwindSafe for AutoResetWaitFuture {}

impl Future for AutoResetWaitFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<()> {
        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };
        let node_ptr = this.node.get();

        let mut state = this.inner.state.lock().expect(NEVER_POISONED);

        // Check if we were directly notified by set() (it popped us from the
        // list and set our notified flag).
        // SAFETY: We hold the lock.
        if unsafe { (*node_ptr).notified } {
            this.registered = false;
            return Poll::Ready(());
        }

        // Check if the flag is set (set() was called with no waiters).
        if state.is_set {
            state.is_set = false;
            if this.registered {
                // SAFETY: We hold the lock and the node is in the list.
                unsafe {
                    state.waiters.remove(node_ptr);
                }
                this.registered = false;
            }
            return Poll::Ready(());
        }

        // Not ready — register or update waker.
        // SAFETY: We hold the lock.
        unsafe {
            (*node_ptr).waker = Some(cx.waker().clone());
        }

        if !this.registered {
            // SAFETY: We hold the lock, node is pinned and not in any list.
            unsafe {
                state.waiters.push_back(node_ptr);
            }
            this.registered = true;
        }

        Poll::Pending
    }
}

impl Drop for AutoResetWaitFuture {
    fn drop(&mut self) {
        if !self.registered {
            return;
        }

        let node_ptr = self.node.get();
        let mut state = self.inner.state.lock().expect(NEVER_POISONED);

        // SAFETY: We hold the lock.
        if unsafe { (*node_ptr).notified } {
            // We were notified but the future was cancelled before it could
            // complete. Forward the notification to the next waiter so that
            // no signal is lost.
            // SAFETY: We hold the lock.
            if let Some(next_node) = unsafe { state.waiters.pop_front() } {
                // SAFETY: We hold the lock and just popped this node.
                unsafe {
                    (*next_node).notified = true;
                }
                // SAFETY: Same node, we hold the lock.
                let waker = unsafe { (*next_node).waker.take() };
                drop(state);

                if let Some(w) = waker {
                    w.wake();
                }
            } else {
                // No more waiters — re-set the flag so the signal is not lost.
                state.is_set = true;
            }
        } else {
            // Not notified — just remove from the list.
            // SAFETY: We hold the lock and the node is in the list.
            unsafe {
                state.waiters.remove(node_ptr);
            }
        }
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

/// Container for embedding an [`AutoResetEvent`]'s state directly in a
/// struct, avoiding the heap allocation that [`AutoResetEvent::boxed()`]
/// requires.
///
/// Create the container with [`new()`][Self::new], pin it, then call
/// [`AutoResetEvent::embedded()`] to obtain a [`RawAutoResetEvent`]
/// handle.
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
/// // SAFETY: The container outlives the handle.
/// let event = unsafe { AutoResetEvent::embedded(container.as_ref()) };
/// let setter = event;
///
/// setter.set();
/// event.wait().await;
/// # });
/// ```
pub struct EmbeddedAutoResetEvent {
    inner: Inner,
    _pinned: PhantomPinned,
}

impl EmbeddedAutoResetEvent {
    /// Creates a new embedded event container in the unset state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Inner {
                state: Mutex::new(GuardedState {
                    is_set: false,
                    waiters: WaiterList::new(),
                }),
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

/// Handle to an embedded [`AutoResetEvent`] created via
/// [`AutoResetEvent::embedded()`].
///
/// This handle uses a raw pointer to the embedded state instead of an
/// [`Arc`]. The caller is responsible for ensuring the
/// [`EmbeddedAutoResetEvent`] outlives all handles and wait futures.
///
/// The API is identical to [`AutoResetEvent`].
#[derive(Clone, Copy)]
pub struct RawAutoResetEvent {
    inner: NonNull<Inner>,
}

#[cfg_attr(coverage_nightly, coverage(off))] // Marker trait impl.
// SAFETY: Inner is Send + Sync (protected by Mutex). The raw pointer is only
// dereferenced to obtain &Inner, which is safe to share across threads.
unsafe impl Send for RawAutoResetEvent {}

#[cfg_attr(coverage_nightly, coverage(off))] // Marker trait impl.
// SAFETY: Same as Send — all mutable access is mediated by the Mutex.
unsafe impl Sync for RawAutoResetEvent {}

#[cfg_attr(coverage_nightly, coverage(off))] // Marker trait impl.
impl UnwindSafe for RawAutoResetEvent {}
#[cfg_attr(coverage_nightly, coverage(off))] // Marker trait impl.
impl RefUnwindSafe for RawAutoResetEvent {}

impl RawAutoResetEvent {
    fn inner(&self) -> &Inner {
        // SAFETY: The caller of `embedded()` guarantees the container
        // outlives this handle.
        unsafe { self.inner.as_ref() }
    }

    /// Signals the event, releasing exactly one waiter.
    // Mutating set() to a no-op causes wait futures to hang.
    #[cfg_attr(test, mutants::skip)]
    pub fn set(&self) {
        let waker: Option<Waker>;

        {
            let mut state = self.inner().state.lock().expect(NEVER_POISONED);

            // SAFETY: We hold the lock, so all node pointers are valid.
            if let Some(node_ptr) = unsafe { state.waiters.pop_front() } {
                // Notify the next waiter.
                // SAFETY: We hold the lock and just popped this node.
                unsafe {
                    (*node_ptr).notified = true;
                }

                // SAFETY: Same node, we hold the lock.
                waker = unsafe { (*node_ptr).waker.take() };
            } else {
                // No waiters — store the signal for the next waiter.
                state.is_set = true;
                waker = None;
            }
        }

        if let Some(w) = waker {
            w.wake();
        }
    }

    /// Attempts to consume the signal without blocking.
    ///
    /// Returns `true` if the event was set, atomically transitioning it
    /// back to the unset state. Returns `false` if the event was not set.
    #[must_use]
    // Mutating try_acquire() to return false causes spin-loop tests to hang.
    #[cfg_attr(test, mutants::skip)]
    pub fn try_acquire(&self) -> bool {
        let mut state = self.inner().state.lock().expect(NEVER_POISONED);
        if state.is_set {
            state.is_set = false;
            true
        } else {
            false
        }
    }

    /// Returns a future that completes when the event is signaled.
    #[must_use]
    pub fn wait(&self) -> RawAutoResetWaitFuture {
        RawAutoResetWaitFuture {
            inner: self.inner,
            node: UnsafeCell::new(WaiterNode::new()),
            registered: false,
            _pinned: PhantomPinned,
        }
    }
}

/// Future returned by [`RawAutoResetEvent::wait()`].
pub struct RawAutoResetWaitFuture {
    inner: NonNull<Inner>,
    node: UnsafeCell<WaiterNode>,
    registered: bool,
    _pinned: PhantomPinned,
}

#[cfg_attr(coverage_nightly, coverage(off))] // Marker trait impl.
// SAFETY: Same reasoning as AutoResetWaitFuture — all node access is
// protected by the Mutex.
unsafe impl Send for RawAutoResetWaitFuture {}

#[cfg_attr(coverage_nightly, coverage(off))] // Marker trait impl.
impl UnwindSafe for RawAutoResetWaitFuture {}
#[cfg_attr(coverage_nightly, coverage(off))] // Marker trait impl.
impl RefUnwindSafe for RawAutoResetWaitFuture {}

impl Future for RawAutoResetWaitFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<()> {
        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };
        // SAFETY: The container outlives this future per the embedded()
        // contract.
        let inner = unsafe { this.inner.as_ref() };
        let node_ptr = this.node.get();

        let mut state = inner.state.lock().expect(NEVER_POISONED);

        // Check if we were directly notified by set() (it popped us from the
        // list and set our notified flag).
        // SAFETY: We hold the lock.
        if unsafe { (*node_ptr).notified } {
            this.registered = false;
            return Poll::Ready(());
        }

        // Check if the flag is set (set() was called with no waiters).
        if state.is_set {
            state.is_set = false;
            if this.registered {
                // SAFETY: We hold the lock and the node is in the list.
                unsafe {
                    state.waiters.remove(node_ptr);
                }
                this.registered = false;
            }
            return Poll::Ready(());
        }

        // Not ready — register or update waker.
        // SAFETY: We hold the lock.
        unsafe {
            (*node_ptr).waker = Some(cx.waker().clone());
        }

        if !this.registered {
            // SAFETY: We hold the lock, node is pinned and not in any list.
            unsafe {
                state.waiters.push_back(node_ptr);
            }
            this.registered = true;
        }

        Poll::Pending
    }
}

impl Drop for RawAutoResetWaitFuture {
    fn drop(&mut self) {
        if !self.registered {
            return;
        }

        let node_ptr = self.node.get();
        // SAFETY: The container outlives this future.
        let inner = unsafe { self.inner.as_ref() };
        let mut state = inner.state.lock().expect(NEVER_POISONED);

        // SAFETY: We hold the lock.
        if unsafe { (*node_ptr).notified } {
            // We were notified but the future was cancelled before it could
            // complete. Forward the notification to the next waiter so that
            // no signal is lost.
            // SAFETY: We hold the lock.
            if let Some(next_node) = unsafe { state.waiters.pop_front() } {
                // SAFETY: We hold the lock and just popped this node.
                unsafe {
                    (*next_node).notified = true;
                }
                // SAFETY: Same node, we hold the lock.
                let waker = unsafe { (*next_node).waker.take() };
                drop(state);

                if let Some(w) = waker {
                    w.wake();
                }
            } else {
                // No more waiters — re-set the flag so the signal is not lost.
                state.is_set = true;
            }
        } else {
            // Not notified — just remove from the list.
            // SAFETY: We hold the lock and the node is in the list.
            unsafe {
                state.waiters.remove(node_ptr);
            }
        }
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
        assert!(!event.try_acquire());
    }

    #[test]
    fn set_then_try_acquire() {
        let event = AutoResetEvent::boxed();
        event.set();
        assert!(event.try_acquire());
        // Signal consumed.
        assert!(!event.try_acquire());
    }

    #[test]
    fn clone_shares_state() {
        let a = AutoResetEvent::boxed();
        let b = a.clone();
        a.set();
        assert!(b.try_acquire());
    }

    #[test]
    fn double_set_without_waiter_only_stores_one_signal() {
        let event = AutoResetEvent::boxed();
        event.set();
        event.set();
        assert!(event.try_acquire());
        // Second set was a no-op (already set).
        assert!(!event.try_acquire());
    }

    // --- async tests ---

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn wait_completes_when_already_set() {
        let event = AutoResetEvent::boxed();
        event.set();
        event.wait().await;
        // Signal consumed.
        assert!(!event.try_acquire());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn wait_completes_after_set() {
        let event = AutoResetEvent::boxed();
        let waiter = event.clone();

        let handle = tokio::spawn(async move {
            waiter.wait().await;
        });

        tokio::task::yield_now().await;
        event.set();
        handle.await.unwrap();
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn only_one_waiter_released_per_set() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let event = AutoResetEvent::boxed();
        let count = Arc::new(AtomicUsize::new(0));

        let mut handles = Vec::new();
        for _ in 0..3 {
            let e = event.clone();
            let c = Arc::clone(&count);
            handles.push(tokio::spawn(async move {
                e.wait().await;
                c.fetch_add(1, Ordering::Relaxed);
            }));
        }

        // Let all three waiters register.
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        // Signal once — exactly one waiter should complete.
        event.set();
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        assert_eq!(count.load(Ordering::Relaxed), 1);

        // Signal twice more to release the remaining two.
        event.set();
        event.set();

        for h in handles {
            h.await.unwrap();
        }

        assert_eq!(count.load(Ordering::Relaxed), 3);
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn cancelled_waiter_forwards_notification() {
        let event = AutoResetEvent::boxed();

        // Spawn a waiter that will be cancelled.
        let e1 = event.clone();
        let handle = tokio::spawn(async move {
            tokio::select! {
                () = e1.wait() => panic!("should be cancelled"),
                () = tokio::task::yield_now() => {}
            }
        });

        handle.await.unwrap();

        // Now set and wait again — should work because cancellation does not
        // lose the signal (the cancelled waiter was never notified).
        event.set();
        event.wait().await;
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn drop_unpolled_future_is_safe() {
        let event = AutoResetEvent::boxed();
        {
            let _future = event.wait();
        }
        event.set();
        event.wait().await;
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

            while !event.try_acquire() {
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
                        if e.try_acquire() {
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
                    while !e.try_acquire() {
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

            while !event.try_acquire() {
                std::hint::spin_loop();
            }

            handle.join().unwrap();
        });
    }

    // --- embedded variant tests ---

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn embedded_set_and_wait() {
        let container = Box::pin(EmbeddedAutoResetEvent::new());
        // SAFETY: The container outlives the handle within this test.
        let event = unsafe { AutoResetEvent::embedded(container.as_ref()) };

        event.set();
        event.wait().await;
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn embedded_clone_shares_state() {
        let container = Box::pin(EmbeddedAutoResetEvent::new());
        // SAFETY: The container outlives the handle within this test.
        let a = unsafe { AutoResetEvent::embedded(container.as_ref()) };
        let b = a;

        a.set();
        assert!(b.try_acquire());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn embedded_signal_consumed() {
        let container = Box::pin(EmbeddedAutoResetEvent::new());
        // SAFETY: The container outlives the handle within this test.
        let event = unsafe { AutoResetEvent::embedded(container.as_ref()) };

        event.set();
        assert!(event.try_acquire());
        // Signal was consumed.
        assert!(!event.try_acquire());
    }

    #[cfg_attr(miri, ignore)]
    #[tokio::test]
    async fn embedded_drop_future_while_waiting() {
        let container = Box::pin(EmbeddedAutoResetEvent::new());
        // SAFETY: The container outlives the handle within this test.
        let event = unsafe { AutoResetEvent::embedded(container.as_ref()) };

        {
            let _future = event.wait();
        }
        event.set();
        event.wait().await;
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

        assert!(event.try_acquire());
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
        let waker = tracker.waker();
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
        assert!(event.try_acquire());
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
        assert!(event.try_acquire());
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
        let waker = tracker.waker();
        let mut cx = task::Context::from_waker(&waker);

        let mut future = Box::pin(event.wait());
        assert!(future.as_mut().poll(&mut cx).is_pending());

        event.set();

        assert!(tracker.was_woken());
    }
}
