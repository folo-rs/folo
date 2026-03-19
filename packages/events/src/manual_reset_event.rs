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

/// Clones the waker from a waiter node and pushes it into the collector.
///
/// # Safety
///
/// The caller must guarantee that `node` points to a valid [`WaiterNode`]
/// (e.g. by holding the owning event's lock).
fn collect_waker(wakers: &mut Vec<Waker>, node: *mut WaiterNode) {
    // SAFETY: Caller guarantees the node pointer is valid.
    if let Some(waker) = unsafe { (*node).waker.as_ref() } {
        wakers.push(waker.clone());
    }
}

/// Thread-safe async eventthat, once set, releases all current and future
/// awaiters until explicitly reset.
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
/// # futures::executor::block_on(async {
/// let event = ManualResetEvent::boxed();
/// let waiter = event.clone();
///
/// // Gate is closed — wait would block, but set opens it first.
/// event.set();
///
/// // Returns immediately because the gate is open.
/// waiter.wait().await;
///
/// // Close the gate again.
/// event.reset();
///
/// assert!(!event.is_set());
/// # });
/// ```
#[derive(Clone)]
pub struct ManualResetEvent {
    inner: Arc<Inner>,
}

struct Inner {
    state: Mutex<GuardedState>,
}

struct GuardedState {
    is_set: bool,
    waiters: WaiterList,
}

// SAFETY: The raw pointers inside WaiterList are only dereferenced while the
// Mutex is held, ensuring exclusive access.
unsafe impl Send for GuardedState {}

impl ManualResetEvent {
    /// Creates a new event in the unset state.
    ///
    /// The returned handle is backed by a heap-allocated shared state
    /// ([`Arc`]). Clone the handle to obtain additional references to the
    /// same event.
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
    /// assert!(event.is_set());
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

    /// Creates a handle backed by an [`EmbeddedManualResetEvent`] container
    /// instead of a heap-allocated [`Arc`].
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
    /// use std::pin::Pin;
    /// use events::{EmbeddedManualResetEvent, ManualResetEvent};
    ///
    /// # futures::executor::block_on(async {
    /// let container = Box::pin(EmbeddedManualResetEvent::new());
    ///
    /// // SAFETY: The container outlives the handle.
    /// let event = unsafe { ManualResetEvent::embedded(container.as_ref()) };
    ///
    /// event.set();
    /// event.wait().await;
    /// # });
    /// ```
    #[must_use]
    pub unsafe fn embedded(place: Pin<&EmbeddedManualResetEvent>) -> RawManualResetEvent {
        let inner = NonNull::from(&place.get_ref().inner);
        RawManualResetEvent { inner }
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
    /// # futures::executor::block_on(async {
    /// let event = ManualResetEvent::boxed();
    /// event.set();
    /// event.wait().await; // completes immediately
    /// # });
    /// ```
    pub fn set(&self) {
        let mut wakers: Vec<Waker> = Vec::new();

        {
            let mut state = self.inner.state.lock().expect(NEVER_POISONED);
            state.is_set = true;

            // Collect wakers from all registered waiters so we can wake them
            // after releasing the lock (preventing re-entrancy deadlocks).
            // SAFETY: We hold the lock, so the list traversal is safe.
            unsafe {
                state.waiters.for_each(|node| {
                    collect_waker(&mut wakers, node);
                });
            }
        }

        for waker in wakers {
            waker.wake();
        }
    }

    /// Closes the gate. Future calls to [`wait()`][Self::wait] will block
    /// until the next [`set()`][Self::set].
    ///
    /// Awaiters that are already past the gate (i.e. whose futures have
    /// already returned [`Poll::Ready`]) are not affected.
    pub fn reset(&self) {
        let mut state = self.inner.state.lock().expect(NEVER_POISONED);
        state.is_set = false;
    }

    /// Returns `true` if the event is currently set.
    ///
    /// Because other threads may set or reset the event concurrently, the
    /// returned value is immediately stale. Use this for diagnostics or
    /// best-effort checks, not for synchronization.
    #[must_use]
    pub fn is_set(&self) -> bool {
        let state = self.inner.state.lock().expect(NEVER_POISONED);
        state.is_set
    }

    /// Non-blocking check equivalent to [`is_set()`][Self::is_set].
    ///
    /// For `ManualResetEvent` the signal is never consumed, so this behaves
    /// identically to `is_set()`. It is provided for API symmetry with
    /// [`AutoResetEvent::try_acquire()`][crate::AutoResetEvent::try_acquire].
    #[must_use]
    pub fn try_acquire(&self) -> bool {
        self.is_set()
    }

    /// Returns a future that completes when the event is set.
    ///
    /// If the event is already set at the time of polling, the future
    /// completes immediately. If the event is reset between being woken and
    /// being re-polled, the future goes back to pending.
    ///
    /// The returned future borrows nothing — it holds an [`Arc`] clone of
    /// the shared state internally, so it can be sent to other tasks freely.
    ///
    /// # Examples
    ///
    /// ```
    /// use events::ManualResetEvent;
    ///
    /// # futures::executor::block_on(async {
    /// let event = ManualResetEvent::boxed();
    ///
    /// // Spawn multiple waiters.
    /// let e1 = event.clone();
    /// let e2 = event.clone();
    ///
    /// // Open the gate — both waiters will complete.
    /// event.set();
    ///
    /// e1.wait().await;
    /// e2.wait().await;
    /// # });
    /// ```
    #[must_use]
    pub fn wait(&self) -> ManualResetWaitFuture {
        ManualResetWaitFuture {
            inner: Arc::clone(&self.inner),
            node: UnsafeCell::new(WaiterNode::new()),
            registered: false,
            _pinned: PhantomPinned,
        }
    }
}

/// Future returned by [`ManualResetEvent::wait()`].
///
/// Completes with `()` when the event is in the set state at the time of
/// polling. The future is `!Unpin` because it contains an intrusive list node
/// that must remain at a stable address once registered.
pub struct ManualResetWaitFuture {
    inner: Arc<Inner>,

    // Behind UnsafeCell so that raw pointers from the event's waiter list can
    // coexist with the &mut Self we obtain in poll() via get_unchecked_mut().
    // UnsafeCell opts out of the noalias guarantee for its contents.
    node: UnsafeCell<WaiterNode>,

    // Only accessed through &mut Self in poll()/drop(), never through the list.
    registered: bool,

    _pinned: PhantomPinned,
}

// SAFETY: All UnsafeCell<WaiterNode> fields are accessed exclusively under the
// event's Mutex. The Arc<Inner> is Send + Sync. The raw pointers inside
// WaiterNode point to nodes in other futures that may be on other threads, but
// are only dereferenced under the Mutex.
unsafe impl Send for ManualResetWaitFuture {}

// The UnsafeCell<WaiterNode> field causes auto-trait inference to mark the
// future as !UnwindSafe and !RefUnwindSafe. However, a shared reference cannot
// observe inconsistent state because all mutable access to the node goes
// through the Mutex or through Pin<&mut Self> (which is exclusive).
impl UnwindSafe for ManualResetWaitFuture {}
impl RefUnwindSafe for ManualResetWaitFuture {}

impl Future for ManualResetWaitFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<()> {
        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };
        let node_ptr = this.node.get();

        let mut state = this.inner.state.lock().expect(NEVER_POISONED);

        if state.is_set {
            if this.registered {
                // SAFETY: We hold the lock and the node is in the list.
                unsafe {
                    state.waiters.remove(node_ptr);
                }
                this.registered = false;
            }
            return Poll::Ready(());
        }

        // Event is not set — register or update the waker.
        // SAFETY: We hold the lock, so exclusive access to the node is
        // guaranteed.
        unsafe {
            (*node_ptr).waker = Some(cx.waker().clone());
        }

        if !this.registered {
            // SAFETY: We hold the lock, the node is pinned (PhantomPinned +
            // Pin<&mut Self>), and it is not in any list.
            unsafe {
                state.waiters.push_back(node_ptr);
            }
            this.registered = true;
        }

        Poll::Pending
    }
}

impl Drop for ManualResetWaitFuture {
    fn drop(&mut self) {
        if self.registered {
            let mut state = self.inner.state.lock().expect(NEVER_POISONED);

            // SAFETY: We hold the lock. If registered is true, the node is
            // still in the list (ManualResetEvent::set() does not remove
            // nodes from the list).
            unsafe {
                state.waiters.remove(self.node.get());
            }
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract for Debug format.
impl fmt::Debug for ManualResetEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let is_set = self.is_set();
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

/// Container for embedding a [`ManualResetEvent`]'s state directly in a
/// struct, avoiding the heap allocation that [`ManualResetEvent::boxed()`]
/// requires.
///
/// Create the container with [`new()`][Self::new], pin it, then call
/// [`ManualResetEvent::embedded()`] to obtain a [`RawManualResetEvent`]
/// handle.
///
/// # Examples
///
/// ```
/// use std::pin::Pin;
/// use events::{EmbeddedManualResetEvent, ManualResetEvent};
///
/// # futures::executor::block_on(async {
/// let container = Box::pin(EmbeddedManualResetEvent::new());
///
/// // SAFETY: The container outlives the handle.
/// let event = unsafe { ManualResetEvent::embedded(container.as_ref()) };
/// let waiter = event.clone();
///
/// event.set();
/// waiter.wait().await;
/// # });
/// ```
pub struct EmbeddedManualResetEvent {
    inner: Inner,
    _pinned: PhantomPinned,
}

impl EmbeddedManualResetEvent {
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

impl Default for EmbeddedManualResetEvent {
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder to new().
    fn default() -> Self {
        Self::new()
    }
}

/// Handle to an embedded [`ManualResetEvent`] created via
/// [`ManualResetEvent::embedded()`].
///
/// This handle uses a raw pointer to the embedded state instead of an
/// [`Arc`]. The caller is responsible for ensuring the
/// [`EmbeddedManualResetEvent`] outlives all handles and wait futures.
///
/// The API is identical to [`ManualResetEvent`].
#[derive(Clone, Copy)]
pub struct RawManualResetEvent {
    inner: NonNull<Inner>,
}

// SAFETY: Inner is Send + Sync (protected by Mutex). The raw pointer is only
// dereferenced to obtain &Inner, which is safe to share across threads.
unsafe impl Send for RawManualResetEvent {}

// SAFETY: Same as Send — all mutable access is mediated by the Mutex.
unsafe impl Sync for RawManualResetEvent {}

impl UnwindSafe for RawManualResetEvent {}
impl RefUnwindSafe for RawManualResetEvent {}

impl RawManualResetEvent {
    fn inner(&self) -> &Inner {
        // SAFETY: The caller of `embedded()` guarantees the container
        // outlives this handle.
        unsafe { self.inner.as_ref() }
    }

    /// Opens the gate, releasing all current awaiters.
    pub fn set(&self) {
        let mut wakers: Vec<Waker> = Vec::new();

        {
            let mut state = self.inner().state.lock().expect(NEVER_POISONED);
            state.is_set = true;

            // SAFETY: We hold the lock.
            unsafe {
                state.waiters.for_each(|node| {
                    collect_waker(&mut wakers, node);
                });
            }
        }

        for waker in wakers {
            waker.wake();
        }
    }

    /// Closes the gate.
    pub fn reset(&self) {
        let mut state = self.inner().state.lock().expect(NEVER_POISONED);
        state.is_set = false;
    }

    /// Returns `true` if the event is currently set.
    #[must_use]
    pub fn is_set(&self) -> bool {
        let state = self.inner().state.lock().expect(NEVER_POISONED);
        state.is_set
    }

    /// Non-blocking check equivalent to [`is_set()`][Self::is_set].
    #[must_use]
    pub fn try_acquire(&self) -> bool {
        self.is_set()
    }

    /// Returns a future that completes when the event is set.
    #[must_use]
    pub fn wait(&self) -> RawManualResetWaitFuture {
        RawManualResetWaitFuture {
            inner: self.inner,
            node: UnsafeCell::new(WaiterNode::new()),
            registered: false,
            _pinned: PhantomPinned,
        }
    }
}

/// Future returned by [`RawManualResetEvent::wait()`].
pub struct RawManualResetWaitFuture {
    inner: NonNull<Inner>,
    node: UnsafeCell<WaiterNode>,
    registered: bool,
    _pinned: PhantomPinned,
}

// SAFETY: Same reasoning as ManualResetWaitFuture — all node access is
// protected by the Mutex.
unsafe impl Send for RawManualResetWaitFuture {}

impl UnwindSafe for RawManualResetWaitFuture {}
impl RefUnwindSafe for RawManualResetWaitFuture {}

impl Future for RawManualResetWaitFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<()> {
        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };
        // SAFETY: The container outlives this future per the embedded()
        // contract.
        let inner = unsafe { this.inner.as_ref() };
        let node_ptr = this.node.get();

        let mut state = inner.state.lock().expect(NEVER_POISONED);

        if state.is_set {
            if this.registered {
                // SAFETY: We hold the lock and the node is in the list.
                unsafe {
                    state.waiters.remove(node_ptr);
                }
                this.registered = false;
            }
            return Poll::Ready(());
        }

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

impl Drop for RawManualResetWaitFuture {
    fn drop(&mut self) {
        if self.registered {
            // SAFETY: The container outlives this future.
            let inner = unsafe { self.inner.as_ref() };
            let mut state = inner.state.lock().expect(NEVER_POISONED);

            // SAFETY: We hold the lock and the node is in the list.
            unsafe {
                state.waiters.remove(self.node.get());
            }
        }
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
        let is_set = self.is_set();
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
    use std::iter;
    use std::sync::Barrier;
    use std::thread;

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
        assert!(!event.is_set());
        assert!(!event.try_acquire());
    }

    #[test]
    fn set_makes_is_set_true() {
        let event = ManualResetEvent::boxed();
        event.set();
        assert!(event.is_set());
    }

    #[test]
    fn reset_after_set() {
        let event = ManualResetEvent::boxed();
        event.set();
        event.reset();
        assert!(!event.is_set());
    }

    #[test]
    fn clone_shares_state() {
        let a = ManualResetEvent::boxed();
        let b = a.clone();
        a.set();
        assert!(b.is_set());
    }

    // --- async tests ---

    #[tokio::test]
    async fn wait_completes_when_already_set() {
        let event = ManualResetEvent::boxed();
        event.set();
        event.wait().await;
    }

    #[tokio::test]
    async fn wait_completes_after_set() {
        let event = ManualResetEvent::boxed();
        let waiter = event.clone();

        let handle = tokio::spawn(async move {
            waiter.wait().await;
        });

        // Give the waiter a moment to register.
        tokio::task::yield_now().await;

        event.set();
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn multiple_waiters_all_released() {
        let event = ManualResetEvent::boxed();

        let mut handles = Vec::new();
        for _ in 0..5 {
            let e = event.clone();
            handles.push(tokio::spawn(async move {
                e.wait().await;
            }));
        }

        tokio::task::yield_now().await;
        event.set();

        for h in handles {
            h.await.unwrap();
        }
    }

    #[tokio::test]
    async fn drop_future_while_waiting() {
        let event = ManualResetEvent::boxed();
        {
            let _future = event.wait();
            // future is dropped here without being polled — should not leak
        }
        // Event should still work fine.
        event.set();
        event.wait().await;
    }

    #[tokio::test]
    async fn drop_polled_future_while_waiting() {
        let event = ManualResetEvent::boxed();

        let waiter = event.clone();
        let handle = tokio::spawn(async move {
            // This polls at least once (registering in the waiter list).
            tokio::select! {
                () = waiter.wait() => panic!("should not complete"),
                () = tokio::task::yield_now() => {}
            }
            // Future is dropped here while registered — should unlink cleanly.
        });

        handle.await.unwrap();

        // Event should still work after the cancelled waiter is gone.
        event.set();
        event.wait().await;
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
            while !event.is_set() {
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

                    while !e.is_set() {
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

            while !event.is_set() {
                std::hint::spin_loop();
            }

            handle.join().unwrap();
        });
    }

    // --- embedded variant tests ---

    #[tokio::test]
    async fn embedded_set_and_wait() {
        let container = Box::pin(EmbeddedManualResetEvent::new());
        // SAFETY: The container outlives the handle within this test.
        let event = unsafe { ManualResetEvent::embedded(container.as_ref()) };

        event.set();
        event.wait().await;
    }

    #[tokio::test]
    async fn embedded_clone_shares_state() {
        let container = Box::pin(EmbeddedManualResetEvent::new());
        // SAFETY: The container outlives the handle within this test.
        let a = unsafe { ManualResetEvent::embedded(container.as_ref()) };
        let b = a;

        a.set();
        assert!(b.is_set());
        b.wait().await;
    }

    #[tokio::test]
    async fn embedded_reset_after_set() {
        let container = Box::pin(EmbeddedManualResetEvent::new());
        // SAFETY: The container outlives the handle within this test.
        let event = unsafe { ManualResetEvent::embedded(container.as_ref()) };

        event.set();
        event.reset();
        assert!(!event.is_set());
    }

    #[tokio::test]
    async fn embedded_drop_future_while_waiting() {
        let container = Box::pin(EmbeddedManualResetEvent::new());
        // SAFETY: The container outlives the handle within this test.
        let event = unsafe { ManualResetEvent::embedded(container.as_ref()) };

        {
            let _future = event.wait();
        }
        event.set();
        event.wait().await;
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
}
