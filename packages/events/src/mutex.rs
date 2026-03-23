use std::cell::UnsafeCell;
use std::fmt;
use std::future::Future;
use std::marker::PhantomPinned;
use std::ops::{Deref, DerefMut};
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex as StdMutex};
use std::task::{self, Poll, Waker};

use crate::NEVER_POISONED;
use crate::waiter_list::{WaiterList, WaiterNode};

/// Thread-safe async mutex.
///
/// Provides mutual exclusion for a shared value of type `T`. The
/// [`lock()`][Self::lock] method returns a future that resolves to a
/// [`MutexGuard`] providing [`Deref`] and [`DerefMut`] access to the
/// protected value. The guard releases the lock when dropped.
///
/// Unlike [`std::sync::Mutex`], this mutex does not block the current
/// thread. Instead, it parks the calling future and wakes it when the
/// lock becomes available, making it suitable for use in async contexts.
///
/// The mutex is a lightweight cloneable handle. All clones derived from
/// the same [`boxed()`][Self::boxed] call share the same underlying
/// state.
///
/// # Fairness
///
/// Waiters are served in FIFO order. When a lock holder unlocks while
/// waiters are queued, the lock is transferred directly to the
/// longest-waiting future, preventing starvation.
///
/// # Cancellation safety
///
/// If a lock future that has been notified is dropped before it is
/// polled to completion, the lock is forwarded to the next waiter (or
/// released if no waiters remain). No lock acquisition is lost due to
/// cancellation.
///
/// # Examples
///
/// ```
/// use events::Mutex;
///
/// #[tokio::main]
/// async fn main() {
///     let mutex = Mutex::boxed(0_u32);
///     let handle = mutex.clone();
///
///     tokio::spawn(async move {
///         let mut guard = handle.lock().await;
///         *guard += 1;
///     });
///
///     let guard = mutex.lock().await;
///     // Value is either 0 or 1, depending on scheduling.
///     assert!(*guard <= 1);
/// }
/// ```
pub struct Mutex<T> {
    inner: Arc<MutexInner<T>>,
}

impl<T> Clone for Mutex<T> {
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

struct MutexInner<T> {
    lock_state: StdMutex<LockState>,
    data: UnsafeCell<T>,
}

struct LockState {
    locked: bool,
    waiters: WaiterList,
}

// SAFETY: All access to the `UnsafeCell<T>` goes through the lock,
// ensuring mutual exclusion. The `StdMutex<LockState>` handles
// thread-safe access to the waiter list. `T: Send` is required
// because the mutex can transfer `T` access from one thread to
// another.
// Marker trait impl.
unsafe impl<T: Send> Sync for MutexInner<T> {}

// SAFETY: `LockState` contains raw pointers (via `WaiterList`) which
// are `!Send` by default. Sending is safe because the pointers are
// only dereferenced while the `StdMutex` is held.
// Marker trait impl.
unsafe impl Send for LockState {}

// The `UnsafeCell<T>` causes auto-trait inference to mark `MutexInner`
// as `!UnwindSafe` and `!RefUnwindSafe`. However, all mutable access
// to the data goes through the lock, preventing inconsistent state
// observation.
// Marker trait impl.
impl<T> UnwindSafe for MutexInner<T> {}
// Marker trait impl.
impl<T> RefUnwindSafe for MutexInner<T> {}

// Mutating unlock() to a no-op causes lock futures to hang.
#[cfg_attr(test, mutants::skip)]
fn unlock(lock_state: &StdMutex<LockState>) {
    let waker: Option<Waker>;

    {
        let mut state = lock_state.lock().expect(NEVER_POISONED);

        // SAFETY: We hold the lock.
        if let Some(node_ptr) = unsafe { state.waiters.pop_front() } {
            // Transfer lock ownership to the next waiter. The lock
            // stays held — the new owner will create its guard on the
            // next poll.
            // SAFETY: We hold the lock and just popped this node.
            unsafe {
                (*node_ptr).notified = true;
            }

            // SAFETY: Same node, we hold the lock.
            waker = unsafe { (*node_ptr).waker.take() };
        } else {
            // No waiters — release the lock.
            state.locked = false;
            waker = None;
        }
    }

    if let Some(w) = waker {
        w.wake();
    }
}

// Mutating try_lock_inner to always return false breaks tests.
#[cfg_attr(test, mutants::skip)]
fn try_lock_inner(lock_state: &StdMutex<LockState>) -> bool {
    let mut state = lock_state.lock().expect(NEVER_POISONED);
    if !state.locked {
        state.locked = true;
        true
    } else {
        false
    }
}

/// Shared poll logic for both `MutexLockFuture` and
/// `RawMutexLockFuture`.
///
/// # Safety
///
/// * The `node` must be pinned and must remain at the same memory
///   address for the lifetime of the lock future.
/// * The `lock_state` must protect the waiter list that this node is
///   (or will be) registered with.
unsafe fn poll_lock(
    lock_state: &StdMutex<LockState>,
    node: &UnsafeCell<WaiterNode>,
    registered: &mut bool,
    waker: Waker,
) -> Poll<()> {
    let node_ptr = node.get();

    let mut state = lock_state.lock().expect(NEVER_POISONED);

    // Check if we were directly notified by unlock() (it popped us
    // from the list and set our notified flag, transferring lock
    // ownership to us).
    // SAFETY: We hold the lock.
    if unsafe { (*node_ptr).notified } {
        *registered = false;
        return Poll::Ready(());
    }

    if !state.locked {
        // Lock is free — acquire it.
        debug_assert!(
            !*registered,
            "unlocked state is exclusive with registered waiters"
        );
        state.locked = true;
        Poll::Ready(())
    } else {
        // Lock is held — register as a waiter.
        // SAFETY: We hold the lock.
        unsafe {
            (*node_ptr).waker = Some(waker);
        }
        if !*registered {
            // SAFETY: We hold the lock, node is pinned and not
            // in any list.
            unsafe {
                state.waiters.push_back(node_ptr);
            }
            *registered = true;
        }
        Poll::Pending
    }
}

/// Shared drop logic for both lock future types.
///
/// # Safety
///
/// Same requirements as [`poll_lock`].
unsafe fn drop_lock_wait(
    lock_state: &StdMutex<LockState>,
    node: &UnsafeCell<WaiterNode>,
    registered: bool,
) {
    // The caller must only call this when the node is registered.
    debug_assert!(registered);

    let node_ptr = node.get();
    let mut state = lock_state.lock().expect(NEVER_POISONED);

    // SAFETY: We hold the lock.
    if unsafe { (*node_ptr).notified } {
        // We were chosen as the next lock holder but the future was
        // cancelled. Forward the lock to the next waiter.
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
            // No more waiters — release the lock.
            state.locked = false;
        }
    } else {
        // Not notified — just remove from the waiter list.
        // SAFETY: We hold the lock and the node is in the list.
        unsafe {
            state.waiters.remove(node_ptr);
        }
    }
}

impl<T> Mutex<T> {
    /// Creates a new mutex wrapping the given value.
    ///
    /// The state is heap-allocated. Clone the handle to share the same
    /// mutex. For caller-provided storage, see
    /// [`embedded()`][Self::embedded].
    ///
    /// # Examples
    ///
    /// ```
    /// use events::Mutex;
    ///
    /// let mutex = Mutex::boxed(42);
    /// let clone = mutex.clone();
    ///
    /// // Both handles operate on the same underlying mutex.
    /// assert_eq!(*clone.try_lock().unwrap(), 42);
    /// ```
    #[must_use]
    pub fn boxed(value: T) -> Self {
        Self {
            inner: Arc::new(MutexInner {
                lock_state: StdMutex::new(LockState {
                    locked: false,
                    waiters: WaiterList::new(),
                }),
                data: UnsafeCell::new(value),
            }),
        }
    }

    /// Creates a handle from an [`EmbeddedMutex`] container, avoiding
    /// heap allocation.
    ///
    /// Calling this multiple times on the same container is safe and
    /// returns handles that all operate on the same shared state, just
    /// like copying or cloning a [`RawMutex`].
    ///
    /// # Safety
    ///
    /// The caller must ensure that the [`EmbeddedMutex`] outlives all
    /// returned handles, all [`RawMutexLockFuture`]s, and all
    /// [`RawMutexGuard`]s created from them.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::pin::pin;
    ///
    /// use events::{EmbeddedMutex, Mutex};
    ///
    /// # futures::executor::block_on(async {
    /// let container = pin!(EmbeddedMutex::new(0_u32));
    ///
    /// // SAFETY: The container outlives the handle and all guards.
    /// let mutex = unsafe { Mutex::embedded(container.as_ref()) };
    ///
    /// let mut guard = mutex.lock().await;
    /// *guard += 1;
    /// assert_eq!(*guard, 1);
    /// # });
    /// ```
    #[must_use]
    pub unsafe fn embedded(place: Pin<&EmbeddedMutex<T>>) -> RawMutex<T> {
        let inner = NonNull::from(&place.get_ref().inner);
        RawMutex { inner }
    }

    /// Returns a future that resolves to a [`MutexGuard`] when the
    /// lock is acquired.
    ///
    /// If the mutex is currently unlocked, the returned future
    /// completes immediately on first poll. Otherwise it parks until
    /// the current holder (and any earlier waiters) release the lock.
    ///
    /// # Cancellation safety
    ///
    /// If a future that has been notified is dropped before it is
    /// polled to completion, the lock is forwarded to the next waiter
    /// (or released if no waiters remain). No lock acquisition is lost
    /// due to cancellation.
    ///
    /// # Examples
    ///
    /// ```
    /// use events::Mutex;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let mutex = Mutex::boxed(String::new());
    ///     let handle = mutex.clone();
    ///
    ///     tokio::spawn(async move {
    ///         let mut guard = handle.lock().await;
    ///         guard.push_str("hello");
    ///     });
    ///
    ///     let guard = mutex.lock().await;
    ///     assert!(guard.is_empty() || *guard == "hello");
    /// }
    /// ```
    #[must_use]
    pub fn lock(&self) -> MutexLockFuture<'_, T> {
        MutexLockFuture {
            lock_state: &self.inner.lock_state,
            data: &self.inner.data,
            node: UnsafeCell::new(WaiterNode::new()),
            registered: false,
            _pinned: PhantomPinned,
        }
    }

    /// Attempts to acquire the lock without blocking.
    ///
    /// Returns [`Some(MutexGuard)`][MutexGuard] if the lock was
    /// successfully acquired, or [`None`] if it is currently held.
    ///
    /// # Examples
    ///
    /// ```
    /// use events::Mutex;
    ///
    /// let mutex = Mutex::boxed(42);
    ///
    /// let guard = mutex.try_lock().unwrap();
    /// assert_eq!(*guard, 42);
    ///
    /// // Lock is held — try_lock returns None.
    /// assert!(mutex.try_lock().is_none());
    /// ```
    #[must_use]
    // Mutating try_lock to always return None breaks tests.
    #[cfg_attr(test, mutants::skip)]
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        if try_lock_inner(&self.inner.lock_state) {
            Some(MutexGuard {
                lock_state: &self.inner.lock_state,
                data: &self.inner.data,
            })
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// MutexGuard
// ---------------------------------------------------------------------------

/// RAII guard returned by [`Mutex::lock()`] and [`Mutex::try_lock()`].
///
/// Provides [`Deref`] and [`DerefMut`] access to the mutex-protected
/// value. The lock is released when the guard is dropped.
pub struct MutexGuard<'a, T> {
    lock_state: &'a StdMutex<LockState>,
    data: &'a UnsafeCell<T>,
}

// SAFETY: A `MutexGuard` can be sent to another thread when `T: Send`
// because the mutex ensures only one guard exists at a time.
// Marker trait impl.
unsafe impl<T: Send> Send for MutexGuard<'_, T> {}

// SAFETY: Sharing `&MutexGuard` across threads gives `&T`, which
// requires `T: Sync`.
// Marker trait impl.
unsafe impl<T: Send + Sync> Sync for MutexGuard<'_, T> {}

// Marker trait impl.
impl<T> UnwindSafe for MutexGuard<'_, T> {}
// Marker trait impl.
impl<T> RefUnwindSafe for MutexGuard<'_, T> {}

impl<T> Deref for MutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: We hold the lock, guaranteeing exclusive access to
        // the data. No other guard can access the UnsafeCell while
        // this guard exists.
        unsafe { &*self.data.get() }
    }
}

impl<T> DerefMut for MutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: We hold the lock and have &mut self, guaranteeing
        // exclusive mutable access.
        unsafe { &mut *self.data.get() }
    }
}

impl<T> Drop for MutexGuard<'_, T> {
    // Mutating drop to a no-op would cause the lock to never release.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        unlock(self.lock_state);
    }
}

// ---------------------------------------------------------------------------
// MutexLockFuture
// ---------------------------------------------------------------------------

/// Future returned by [`Mutex::lock()`].
///
/// Completes with a [`MutexGuard`] when the lock is acquired.
pub struct MutexLockFuture<'a, T> {
    lock_state: &'a StdMutex<LockState>,
    data: &'a UnsafeCell<T>,

    // Behind UnsafeCell so that raw pointers from the waiter list can
    // coexist with the &mut Self we obtain in poll() via
    // get_unchecked_mut(). UnsafeCell opts out of the noalias
    // guarantee for its contents.
    node: UnsafeCell<WaiterNode>,

    // Whether this future's node is currently in the waiter list.
    // Only accessed through &mut Self in poll()/drop(), never through
    // the list.
    registered: bool,

    _pinned: PhantomPinned,
}

// Marker trait impl.
// SAFETY: All UnsafeCell<WaiterNode> fields are accessed exclusively
// under the mutex's internal lock. The references point to data behind
// an Arc that is Send + Sync when T: Send.
unsafe impl<T: Send> Send for MutexLockFuture<'_, T> {}

// The UnsafeCell<WaiterNode> field causes auto-trait inference to mark
// the future as !UnwindSafe and !RefUnwindSafe. However, all mutable
// access to the node goes through the internal lock, preventing
// inconsistent state observation.
// Marker trait impl.
impl<T> UnwindSafe for MutexLockFuture<'_, T> {}
// Marker trait impl.
impl<T> RefUnwindSafe for MutexLockFuture<'_, T> {}

impl<'a, T> Future for MutexLockFuture<'a, T> {
    type Output = MutexGuard<'a, T>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<MutexGuard<'a, T>> {
        // Clone the waker before acquiring the lock so a panicking
        // clone implementation cannot poison the mutex.
        let waker = cx.waker().clone();

        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };

        // SAFETY: The node is pinned (PhantomPinned) and the
        // lock_state field is the lock this node registers with.
        match unsafe { poll_lock(this.lock_state, &this.node, &mut this.registered, waker) } {
            Poll::Ready(()) => Poll::Ready(MutexGuard {
                lock_state: this.lock_state,
                data: this.data,
            }),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T> Drop for MutexLockFuture<'_, T> {
    fn drop(&mut self) {
        if !self.registered {
            return;
        }

        // SAFETY: The node is pinned (PhantomPinned) and the
        // lock_state field is the lock this node was registered with.
        unsafe { drop_lock_wait(self.lock_state, &self.node, self.registered) }
    }
}

// ---------------------------------------------------------------------------
// Debug impls
// ---------------------------------------------------------------------------

#[cfg_attr(coverage_nightly, coverage(off))]
impl<T> fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mutex").finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl<T> fmt::Debug for MutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MutexGuard").finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl<T> fmt::Debug for MutexLockFuture<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MutexLockFuture")
            .field("registered", &self.registered)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Embedded variant
// ---------------------------------------------------------------------------

/// Embedded-state container for [`Mutex`].
///
/// Stores the mutex state inline, avoiding the heap allocation that
/// [`Mutex::boxed()`] requires. Create the container with
/// [`new()`][Self::new], pin it, then call [`Mutex::embedded()`] to
/// obtain a [`RawMutex`] handle.
///
/// # Examples
///
/// ```
/// use std::pin::pin;
///
/// use events::{EmbeddedMutex, Mutex};
///
/// # futures::executor::block_on(async {
/// let container = pin!(EmbeddedMutex::new(42));
///
/// // SAFETY: The container outlives the handle and all guards.
/// let mutex = unsafe { Mutex::embedded(container.as_ref()) };
///
/// let guard = mutex.lock().await;
/// assert_eq!(*guard, 42);
/// # });
/// ```
pub struct EmbeddedMutex<T> {
    inner: MutexInner<T>,
    _pinned: PhantomPinned,
}

impl<T> EmbeddedMutex<T> {
    /// Creates a new embedded mutex container wrapping the given value.
    #[must_use]
    pub fn new(value: T) -> Self {
        Self {
            inner: MutexInner {
                lock_state: StdMutex::new(LockState {
                    locked: false,
                    waiters: WaiterList::new(),
                }),
                data: UnsafeCell::new(value),
            },
            _pinned: PhantomPinned,
        }
    }
}

impl<T> Default for EmbeddedMutex<T>
where
    T: Default,
{
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn default() -> Self {
        Self::new(T::default())
    }
}

/// Handle to an embedded [`Mutex`].
///
/// Created via [`Mutex::embedded()`]. The caller is responsible for
/// ensuring the [`EmbeddedMutex`] outlives all handles, lock futures,
/// and guards.
///
/// The API is identical to [`Mutex`].
#[derive(Clone, Copy)]
pub struct RawMutex<T> {
    inner: NonNull<MutexInner<T>>,
}

// Marker trait impl.
// SAFETY: The `NonNull<MutexInner<T>>` only points to a value whose
// access is serialized by the internal `StdMutex`.
unsafe impl<T: Send> Send for RawMutex<T> {}

// Marker trait impl.
// SAFETY: Same as Send — all mutable access is mediated by the
// internal lock.
unsafe impl<T: Send> Sync for RawMutex<T> {}

// Marker trait impl.
impl<T> UnwindSafe for RawMutex<T> {}
// Marker trait impl.
impl<T> RefUnwindSafe for RawMutex<T> {}

impl<T> RawMutex<T> {
    fn inner(&self) -> &MutexInner<T> {
        // SAFETY: The caller of `embedded()` guarantees the container
        // outlives this handle.
        unsafe { self.inner.as_ref() }
    }

    /// Returns a future that resolves to a [`RawMutexGuard`] when the
    /// lock is acquired.
    #[must_use]
    pub fn lock(&self) -> RawMutexLockFuture<T> {
        RawMutexLockFuture {
            inner: self.inner,
            node: UnsafeCell::new(WaiterNode::new()),
            registered: false,
            _pinned: PhantomPinned,
        }
    }

    /// Attempts to acquire the lock without blocking.
    #[must_use]
    // Mutating try_lock to always return None breaks tests.
    #[cfg_attr(test, mutants::skip)]
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn try_lock(&self) -> Option<RawMutexGuard<T>> {
        if try_lock_inner(&self.inner().lock_state) {
            Some(RawMutexGuard { inner: self.inner })
        } else {
            None
        }
    }
}

/// RAII guard returned by [`RawMutex::lock()`] and
/// [`RawMutex::try_lock()`].
///
/// Provides [`Deref`] and [`DerefMut`] access to the mutex-protected
/// value. The lock is released when the guard is dropped.
pub struct RawMutexGuard<T> {
    inner: NonNull<MutexInner<T>>,
}

// Marker trait impl.
// SAFETY: Same reasoning as MutexGuard — only one guard exists at a
// time, and the lock serializes access.
unsafe impl<T: Send> Send for RawMutexGuard<T> {}

// Marker trait impl.
// SAFETY: Sharing &RawMutexGuard across threads gives &T, requiring
// T: Sync.
unsafe impl<T: Send + Sync> Sync for RawMutexGuard<T> {}

// Marker trait impl.
impl<T> UnwindSafe for RawMutexGuard<T> {}
// Marker trait impl.
impl<T> RefUnwindSafe for RawMutexGuard<T> {}

impl<T> Deref for RawMutexGuard<T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: The embedded() contract guarantees the container
        // outlives this guard.
        let inner = unsafe { self.inner.as_ref() };
        // SAFETY: We hold the lock, so exclusive access to data is
        // guaranteed.
        unsafe { &*inner.data.get() }
    }
}

impl<T> DerefMut for RawMutexGuard<T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: The embedded() contract guarantees the container
        // outlives this guard.
        let inner = unsafe { self.inner.as_ref() };
        // SAFETY: We hold the lock and have &mut self, so exclusive
        // access to data is guaranteed.
        unsafe { &mut *inner.data.get() }
    }
}

impl<T> Drop for RawMutexGuard<T> {
    // Mutating drop to a no-op would cause the lock to never release.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        // SAFETY: The embedded() contract guarantees the container
        // outlives this guard.
        let inner = unsafe { self.inner.as_ref() };
        unlock(&inner.lock_state);
    }
}

/// Future returned by [`RawMutex::lock()`].
///
/// Completes with a [`RawMutexGuard`] when the lock is acquired.
pub struct RawMutexLockFuture<T> {
    inner: NonNull<MutexInner<T>>,

    // See MutexLockFuture for field documentation.
    node: UnsafeCell<WaiterNode>,
    registered: bool,

    _pinned: PhantomPinned,
}

// Marker trait impl.
// SAFETY: Same reasoning as MutexLockFuture — all node access is
// protected by the internal lock.
unsafe impl<T: Send> Send for RawMutexLockFuture<T> {}

// Marker trait impl.
impl<T> UnwindSafe for RawMutexLockFuture<T> {}
// Marker trait impl.
impl<T> RefUnwindSafe for RawMutexLockFuture<T> {}

impl<T> Future for RawMutexLockFuture<T> {
    type Output = RawMutexGuard<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<RawMutexGuard<T>> {
        // Clone the waker before acquiring the lock so a panicking
        // clone implementation cannot poison the mutex.
        let waker = cx.waker().clone();

        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };

        // SAFETY: The container outlives this future per the
        // embedded() contract.
        let inner = unsafe { this.inner.as_ref() };
        // SAFETY: The node is pinned (PhantomPinned) and the
        // lock_state is the lock this node registers with.
        match unsafe { poll_lock(&inner.lock_state, &this.node, &mut this.registered, waker) } {
            Poll::Ready(()) => Poll::Ready(RawMutexGuard { inner: this.inner }),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T> Drop for RawMutexLockFuture<T> {
    fn drop(&mut self) {
        if !self.registered {
            return;
        }

        // SAFETY: The container outlives this future per the
        // embedded() contract.
        let inner = unsafe { self.inner.as_ref() };
        // SAFETY: The node is pinned (PhantomPinned) and the
        // lock_state is the lock this node was registered with.
        unsafe { drop_lock_wait(&inner.lock_state, &self.node, self.registered) }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl<T> fmt::Debug for EmbeddedMutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedMutex").finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl<T> fmt::Debug for RawMutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawMutex").finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl<T> fmt::Debug for RawMutexGuard<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawMutexGuard").finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl<T> fmt::Debug for RawMutexLockFuture<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawMutexLockFuture")
            .field("registered", &self.registered)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Barrier;
    use std::{iter, thread};

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    // --- trait assertions ---

    assert_impl_all!(
        Mutex<u32>: Send, Sync, Clone, UnwindSafe, RefUnwindSafe
    );
    assert_impl_all!(
        MutexGuard<'static, u32>: Send, Sync, UnwindSafe, RefUnwindSafe
    );
    assert_not_impl_any!(MutexGuard<'static, u32>: Clone);
    assert_impl_all!(
        MutexLockFuture<'static, u32>: Send, UnwindSafe, RefUnwindSafe
    );
    assert_not_impl_any!(MutexLockFuture<'static, u32>: Sync, Unpin);

    assert_impl_all!(
        EmbeddedMutex<u32>: Send, Sync, UnwindSafe, RefUnwindSafe
    );
    assert_not_impl_any!(EmbeddedMutex<u32>: Unpin);
    assert_impl_all!(
        RawMutex<u32>: Send, Sync, Clone, Copy,
        UnwindSafe, RefUnwindSafe
    );
    assert_impl_all!(
        RawMutexGuard<u32>: Send, Sync, UnwindSafe, RefUnwindSafe
    );
    assert_not_impl_any!(RawMutexGuard<u32>: Clone);
    assert_impl_all!(
        RawMutexLockFuture<u32>: Send, UnwindSafe, RefUnwindSafe
    );
    assert_not_impl_any!(RawMutexLockFuture<u32>: Sync, Unpin);

    // Guard Sync requires T: Sync. Cell is Send but not Sync.
    assert_not_impl_any!(
        MutexGuard<'static, std::cell::Cell<u32>>: Sync
    );

    // --- basic functionality ---

    #[test]
    fn starts_unlocked() {
        let mutex = Mutex::boxed(42_u32);
        let guard = mutex.try_lock();
        assert!(guard.is_some());
        assert_eq!(*guard.unwrap(), 42);
    }

    #[test]
    fn try_lock_when_locked_returns_none() {
        let mutex = Mutex::boxed(42_u32);
        let _guard = mutex.try_lock().unwrap();
        assert!(mutex.try_lock().is_none());
    }

    #[test]
    fn unlock_allows_try_lock() {
        let mutex = Mutex::boxed(42_u32);
        {
            let _guard = mutex.try_lock().unwrap();
        }
        assert!(mutex.try_lock().is_some());
    }

    #[test]
    fn clone_shares_state() {
        let a = Mutex::boxed(42_u32);
        let b = a.clone();
        let guard = a.try_lock().unwrap();
        assert!(b.try_lock().is_none());
        drop(guard);
        assert_eq!(*b.try_lock().unwrap(), 42);
    }

    #[test]
    fn guard_deref_mut() {
        let mutex = Mutex::boxed(0_u32);
        {
            let mut guard = mutex.try_lock().unwrap();
            *guard = 99;
        }
        assert_eq!(*mutex.try_lock().unwrap(), 99);
    }

    // --- async tests ---

    #[test]
    fn lock_completes_when_unlocked() {
        futures::executor::block_on(async {
            let mutex = Mutex::boxed(42_u32);
            let guard = mutex.lock().await;
            assert_eq!(*guard, 42);
        });
    }

    #[test]
    fn lock_completes_after_unlock() {
        let mutex = Mutex::boxed(42_u32);
        let guard = mutex.try_lock().unwrap();
        let mut future = Box::pin(mutex.lock());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(future.as_mut().poll(&mut cx).is_pending());
        drop(guard);
        assert!(future.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn fifo_ordering() {
        let mutex = Mutex::boxed(0_u32);
        let guard = mutex.try_lock().unwrap();

        let mut f1 = Box::pin(mutex.lock());
        let mut f2 = Box::pin(mutex.lock());
        let mut f3 = Box::pin(mutex.lock());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        // All register.
        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());
        assert!(f3.as_mut().poll(&mut cx).is_pending());

        // Drop the original guard — f1 should be next (FIFO).
        drop(guard);
        let Poll::Ready(g1) = f1.as_mut().poll(&mut cx) else {
            panic!("expected Ready for f1")
        };
        assert!(f2.as_mut().poll(&mut cx).is_pending());
        assert!(f3.as_mut().poll(&mut cx).is_pending());

        // Drop f1's guard to release f2.
        drop(g1);
        let Poll::Ready(g2) = f2.as_mut().poll(&mut cx) else {
            panic!("expected Ready for f2")
        };
        assert!(f3.as_mut().poll(&mut cx).is_pending());

        drop(g2);
        assert!(f3.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn cancelled_waiter_releases_lock() {
        let mutex = Mutex::boxed(0_u32);
        let guard = mutex.try_lock().unwrap();

        let mut f1 = Box::pin(mutex.lock());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());

        // Drop guard — f1 is notified.
        drop(guard);
        // Drop f1 without polling — lock should be released.
        drop(f1);

        assert!(mutex.try_lock().is_some());
    }

    #[test]
    fn notified_then_dropped_forwards_to_next() {
        let mutex = Mutex::boxed(0_u32);
        let guard = mutex.try_lock().unwrap();

        let mut f1 = Box::pin(mutex.lock());
        let mut f2 = Box::pin(mutex.lock());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());

        // Drop guard — f1 is notified.
        drop(guard);
        // Drop f1 without polling — should forward to f2.
        drop(f1);

        assert!(f2.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn drop_unpolled_future_is_safe() {
        let mutex = Mutex::boxed(42_u32);
        {
            let _future = mutex.lock();
        }
        assert!(mutex.try_lock().is_some());
    }

    // --- multithreaded tests (Miri-compatible) ---

    #[test]
    fn lock_from_another_thread() {
        testing::with_watchdog(|| {
            let mutex = Mutex::boxed(0_u32);
            let handle = mutex.clone();
            let barrier = Arc::new(Barrier::new(2));
            let b2 = Arc::clone(&barrier);

            let t = thread::spawn(move || {
                b2.wait();
                let mut guard = futures::executor::block_on(handle.lock());
                *guard = 1;
            });

            barrier.wait();

            // Wait until the other thread has incremented.
            loop {
                if let Some(guard) = mutex.try_lock()
                    && *guard == 1
                {
                    break;
                }
                std::hint::spin_loop();
            }

            t.join().unwrap();
        });
    }

    #[test]
    fn mutual_exclusion() {
        testing::with_watchdog(|| {
            let mutex = Mutex::boxed(0_u32);
            let thread_count = 4;
            let iterations = 100;
            let barrier = Arc::new(Barrier::new(thread_count + 1));

            let handles: Vec<_> = iter::repeat_with(|| {
                let m = mutex.clone();
                let b = Arc::clone(&barrier);
                thread::spawn(move || {
                    b.wait();
                    for _ in 0..iterations {
                        let mut guard = futures::executor::block_on(m.lock());
                        let val = *guard;
                        *guard = val.wrapping_add(1);
                    }
                })
            })
            .take(thread_count)
            .collect();

            barrier.wait();

            for h in handles {
                h.join().unwrap();
            }

            let guard = mutex.try_lock().unwrap();
            assert_eq!(*guard, u32::try_from(thread_count * iterations).unwrap());
        });
    }

    // --- embedded variant tests ---

    #[test]
    fn embedded_lock_and_unlock() {
        futures::executor::block_on(async {
            let container = Box::pin(EmbeddedMutex::new(42_u32));
            // SAFETY: The container outlives all handles.
            let mutex = unsafe { Mutex::embedded(container.as_ref()) };

            let guard = mutex.lock().await;
            assert_eq!(*guard, 42);
        });
    }

    #[test]
    fn embedded_clone_shares_state() {
        let container = Box::pin(EmbeddedMutex::new(42_u32));
        // SAFETY: The container outlives all handles.
        let a = unsafe { Mutex::embedded(container.as_ref()) };
        let b = a;

        let guard = a.try_lock().unwrap();
        assert!(b.try_lock().is_none());
        drop(guard);
        assert_eq!(*b.try_lock().unwrap(), 42);
    }

    #[test]
    fn embedded_guard_deref_mut() {
        futures::executor::block_on(async {
            let container = Box::pin(EmbeddedMutex::new(0_u32));
            // SAFETY: The container outlives all handles.
            let mutex = unsafe { Mutex::embedded(container.as_ref()) };

            {
                let mut guard = mutex.lock().await;
                *guard = 99;
            }
            assert_eq!(*mutex.try_lock().unwrap(), 99);
        });
    }

    #[test]
    fn embedded_drop_future_while_waiting() {
        futures::executor::block_on(async {
            let container = Box::pin(EmbeddedMutex::new(42_u32));
            // SAFETY: The container outlives all handles.
            let mutex = unsafe { Mutex::embedded(container.as_ref()) };

            {
                let _future = mutex.lock();
            }
            let guard = mutex.lock().await;
            assert_eq!(*guard, 42);
        });
    }

    #[test]
    fn embedded_from_another_thread() {
        testing::with_watchdog(|| {
            let container = Box::pin(EmbeddedMutex::new(0_u32));
            // SAFETY: The container outlives all handles.
            let mutex = unsafe { Mutex::embedded(container.as_ref()) };
            let setter = mutex;
            let barrier = Arc::new(Barrier::new(2));
            let b2 = Arc::clone(&barrier);

            let t = thread::spawn(move || {
                b2.wait();
                let mut guard = futures::executor::block_on(setter.lock());
                *guard = 1;
            });

            barrier.wait();

            loop {
                if let Some(guard) = mutex.try_lock()
                    && *guard == 1
                {
                    break;
                }
                std::hint::spin_loop();
            }

            t.join().unwrap();
        });
    }
}
