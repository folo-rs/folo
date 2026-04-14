use std::cell::UnsafeCell;
use std::fmt;
use std::future::Future;
use std::marker::{PhantomData, PhantomPinned};
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::Rc;
use std::task::{self, Poll, Waker};

use awaiter_set::{Awaiter, AwaiterSet};

/// Single-threaded async mutex.
///
/// This is the `!Send` counterpart of [`Mutex`][crate::Mutex]. It
/// avoids atomic operations and locking, making it more efficient on
/// single-threaded executors.
///
/// The mutex is a lightweight cloneable handle. All clones derived
/// from the same [`boxed()`][Self::boxed] call share the same
/// underlying state.
///
/// To avoid the heap allocation, use [`EmbeddedLocalMutex`] with
/// [`embedded()`][Self::embedded] instead.
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
/// use asynchroniz::LocalMutex;
///
/// #[tokio::main]
/// async fn main() {
///     let local = tokio::task::LocalSet::new();
///     local
///         .run_until(async {
///             let mutex = LocalMutex::boxed(0_u32);
///
///             tokio::task::spawn_local({
///                 let mutex = mutex.clone();
///                 async move {
///                     let mut guard = mutex.lock().await;
///                     *guard += 1;
///                 }
///             });
///
///             let guard = mutex.lock().await;
///             assert!(*guard <= 1);
///         })
///         .await;
/// }
/// ```
#[derive(Clone)]
pub struct LocalMutex<T> {
    inner: Rc<Inner<T>>,
}

struct LockState {
    locked: bool,
    waiters: AwaiterSet,
}

struct Inner<T> {
    lock_state: UnsafeCell<LockState>,
    data: UnsafeCell<T>,
    _not_send: PhantomData<*const ()>,
}

impl<T> Inner<T> {
    // Mutating unlock to a no-op causes lock futures to hang.
    #[cfg_attr(test, mutants::skip)]
    fn unlock(&self) {
        // Capture the waker while borrowing the state, then wake
        // after the borrow ends to avoid aliased mutable access if
        // the waker is re-entrant.
        let waker = {
            // SAFETY: Single-threaded access guaranteed by !Send.
            let state = unsafe { &mut *self.lock_state.get() };

            // SAFETY: Single-threaded access.
            if let Some(w) = unsafe { state.waiters.notify_one() } {
                Some(w)
            } else {
                state.locked = false;
                None
            }
        };

        if let Some(w) = waker {
            w.wake();
        }
    }

    // Mutating try_lock to always return false breaks tests.
    #[cfg_attr(test, mutants::skip)]
    fn try_lock(&self) -> bool {
        // SAFETY: Single-threaded access.
        let state = unsafe { &mut *self.lock_state.get() };
        if !state.locked {
            state.locked = true;
            true
        } else {
            false
        }
    }

    /// # Safety
    ///
    /// * The `awaiter` must belong to a future created from the same
    ///   mutex.
    unsafe fn poll_lock(&self, mut awaiter: Pin<&mut Awaiter>, waker: Waker) -> Poll<()> {
        // SAFETY: Single-threaded access.
        if unsafe { awaiter.as_mut().take_notification() } {
            return Poll::Ready(());
        }

        // SAFETY: Single-threaded access.
        let state = unsafe { &mut *self.lock_state.get() };

        if !state.locked {
            debug_assert!(
                // SAFETY: Single-threaded access.
                !unsafe { awaiter.is_registered() },
                "unlocked state is exclusive with registered waiters"
            );
            state.locked = true;
            Poll::Ready(())
        } else {
            // Register or update the waker.
            // SAFETY: Single-threaded, awaiter is pinned and not yet
            // in the set (or already registered for waker update).
            unsafe {
                state.waiters.register(awaiter.as_mut(), waker);
            }
            Poll::Pending
        }
    }

    /// # Safety
    ///
    /// Same requirements as [`poll_lock`][Self::poll_lock].
    unsafe fn drop_lock_wait(&self, mut awaiter: Pin<&mut Awaiter>) {
        // SAFETY: Single-threaded access.
        if !unsafe { awaiter.is_registered() } {
            return;
        }

        // SAFETY: Single-threaded access.
        if unsafe { awaiter.as_ref().is_notified() } {
            // Capture the waker while borrowing the state, then wake
            // after the borrow ends to avoid aliased mutable access
            // if the waker is re-entrant.
            let waker = {
                let state_ptr = self.lock_state.get();
                // SAFETY: Single-threaded access.
                let state = unsafe { &mut *state_ptr };

                // SAFETY: Single-threaded access.
                if let Some(w) = unsafe { state.waiters.notify_one() } {
                    Some(w)
                } else {
                    state.locked = false;
                    None
                }
            };

            if let Some(w) = waker {
                w.wake();
            }
        } else {
            // Not notified — just remove from the set.
            // SAFETY: Single-threaded access.
            let state = unsafe { &mut *self.lock_state.get() };
            // SAFETY: Single-threaded, node is in the set.
            unsafe {
                state.waiters.unregister(awaiter.as_mut());
            }
        }
    }
}

impl<T> LocalMutex<T> {
    /// Creates a new mutex wrapping the given value.
    ///
    /// The state is heap-allocated. Clone the handle to share the same
    /// mutex. For caller-provided storage, see
    /// [`embedded()`][Self::embedded].
    ///
    /// # Examples
    ///
    /// ```
    /// use asynchroniz::LocalMutex;
    ///
    /// let mutex = LocalMutex::boxed(42);
    /// assert_eq!(*mutex.try_lock().unwrap(), 42);
    /// ```
    #[must_use]
    pub fn boxed(value: T) -> Self {
        Self {
            inner: Rc::new(Inner {
                lock_state: UnsafeCell::new(LockState {
                    locked: false,
                    waiters: AwaiterSet::new(),
                }),
                data: UnsafeCell::new(value),
                _not_send: PhantomData,
            }),
        }
    }

    /// Creates an instance that references the state in the
    /// [`EmbeddedLocalMutex`].
    ///
    /// Calling this multiple times on the same container returns
    /// handles that all operate on the same shared state.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the [`EmbeddedLocalMutex`] outlives
    /// all returned handles, all [`EmbeddedLocalMutexLockFuture`]s, and all
    /// [`EmbeddedLocalMutexGuard`]s created from them.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::pin::pin;
    ///
    /// use asynchroniz::{EmbeddedLocalMutex, LocalMutex};
    ///
    /// # futures::executor::block_on(async {
    /// let container = pin!(EmbeddedLocalMutex::new(0_u32));
    ///
    /// // SAFETY: The container outlives the handle and all guards.
    /// let mutex = unsafe { LocalMutex::embedded(container.as_ref()) };
    ///
    /// let mut guard = mutex.lock().await;
    /// *guard += 1;
    /// assert_eq!(*guard, 1);
    /// # });
    /// ```
    #[must_use]
    pub unsafe fn embedded(place: Pin<&EmbeddedLocalMutex<T>>) -> EmbeddedLocalMutexRef<T> {
        let inner = NonNull::from(&place.get_ref().inner);
        EmbeddedLocalMutexRef { inner }
    }

    /// Returns a future that resolves to a [`LocalMutexGuard`] when
    /// the lock is acquired.
    ///
    /// # Cancellation safety
    ///
    /// If a lock future that has been notified is dropped before it is
    /// polled to completion, the lock is forwarded to the next waiter
    /// (or released if no waiters remain).
    ///
    /// # Examples
    ///
    /// ```
    /// use asynchroniz::LocalMutex;
    ///
    /// # futures::executor::block_on(async {
    /// let mutex = LocalMutex::boxed(String::new());
    /// let mut guard = mutex.lock().await;
    /// guard.push_str("hello");
    /// assert_eq!(*guard, "hello");
    /// # });
    /// ```
    #[must_use]
    pub fn lock(&self) -> LocalMutexLockFuture<'_, T> {
        LocalMutexLockFuture {
            inner: &self.inner,
            awaiter: Awaiter::new(),
        }
    }

    /// Attempts to acquire the lock without blocking.
    ///
    /// Returns [`Some(LocalMutexGuard)`][LocalMutexGuard] if the lock
    /// was successfully acquired, or [`None`] if it is currently held.
    ///
    /// # Examples
    ///
    /// ```
    /// use asynchroniz::LocalMutex;
    ///
    /// let mutex = LocalMutex::boxed(42);
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
    pub fn try_lock(&self) -> Option<LocalMutexGuard<'_, T>> {
        if self.inner.try_lock() {
            Some(LocalMutexGuard { inner: &self.inner })
        } else {
            None
        }
    }
}

/// RAII guard returned by [`LocalMutex::lock()`] and
/// [`LocalMutex::try_lock()`].
///
/// Provides [`Deref`] and [`DerefMut`] access to the mutex-protected
/// value. The lock is released when the guard is dropped.
pub struct LocalMutexGuard<'a, T> {
    inner: &'a Inner<T>,
}

impl<T> Deref for LocalMutexGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: We hold the lock, guaranteeing exclusive access.
        // Single-threaded access guaranteed by !Send.
        unsafe { &*self.inner.data.get() }
    }
}

impl<T> DerefMut for LocalMutexGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: We hold the lock and have &mut self.
        unsafe { &mut *self.inner.data.get() }
    }
}

impl<T> Drop for LocalMutexGuard<'_, T> {
    // Mutating drop to a no-op would cause the lock to never release.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        self.inner.unlock();
    }
}

/// Future returned by [`LocalMutex::lock()`].
///
/// Completes with a [`LocalMutexGuard`] when the lock is acquired.
pub struct LocalMutexLockFuture<'a, T> {
    inner: &'a Inner<T>,

    awaiter: Awaiter,
}

impl<'a, T> Future for LocalMutexLockFuture<'a, T> {
    type Output = LocalMutexGuard<'a, T>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<LocalMutexGuard<'a, T>> {
        let waker = cx.waker().clone();

        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };

        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut this.awaiter) };
        // SAFETY: The lock_state field is the lock this awaiter
        // registers with.
        match unsafe { this.inner.poll_lock(awaiter, waker) } {
            Poll::Ready(()) => Poll::Ready(LocalMutexGuard { inner: this.inner }),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T> Drop for LocalMutexLockFuture<'_, T> {
    // Inverting the is_registered() guard causes the Drop to hang
    // because it runs cleanup on an unregistered awaiter.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut self.awaiter) };
        // SAFETY: The lock_state is the lock this awaiter was
        // registered with.
        unsafe {
            self.inner.drop_lock_wait(awaiter);
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl<T> fmt::Debug for LocalMutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalMutex").finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl<T> fmt::Debug for LocalMutexGuard<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalMutexGuard").finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl<T> fmt::Debug for LocalMutexLockFuture<'_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalMutexLockFuture")
            // SAFETY: Debug output is best-effort; no concurrent
            // mutation during formatting.
            .finish_non_exhaustive()
    }
}

/// Inline storage for mutex state, avoiding heap allocation.
///
/// Pin the container, then call [`LocalMutex::embedded()`] to obtain
/// a [`EmbeddedLocalMutexRef`] reference that operates on the embedded state.
///
/// # Examples
///
/// ```
/// use std::pin::pin;
///
/// use asynchroniz::{EmbeddedLocalMutex, LocalMutex};
///
/// # futures::executor::block_on(async {
/// let container = pin!(EmbeddedLocalMutex::new(42));
///
/// // SAFETY: The container outlives the handle and all guards.
/// let mutex = unsafe { LocalMutex::embedded(container.as_ref()) };
///
/// let guard = mutex.lock().await;
/// assert_eq!(*guard, 42);
/// # });
/// ```
pub struct EmbeddedLocalMutex<T> {
    inner: Inner<T>,
    // Pinning is required because references (via EmbeddedLocalMutexRef)
    // hold a NonNull pointer to the inner state.
    _pinned: PhantomPinned,
}

impl<T> EmbeddedLocalMutex<T> {
    /// Creates a new embedded mutex container wrapping the given
    /// value.
    #[must_use]
    pub fn new(value: T) -> Self {
        Self {
            inner: Inner {
                lock_state: UnsafeCell::new(LockState {
                    locked: false,
                    waiters: AwaiterSet::new(),
                }),
                data: UnsafeCell::new(value),
                _not_send: PhantomData,
            },
            _pinned: PhantomPinned,
        }
    }
}

impl<T> Default for EmbeddedLocalMutex<T>
where
    T: Default,
{
    fn default() -> Self {
        Self::new(T::default())
    }
}

/// Reference to an [`EmbeddedLocalMutex`].
///
/// Created via [`LocalMutex::embedded()`]. The caller is responsible
/// for ensuring the [`EmbeddedLocalMutex`] outlives all references,
/// lock futures, and guards.
///
/// Provides the same API as [`LocalMutex`].
#[derive(Clone, Copy)]
pub struct EmbeddedLocalMutexRef<T> {
    inner: NonNull<Inner<T>>,
}

impl<T> EmbeddedLocalMutexRef<T> {
    fn inner(&self) -> &Inner<T> {
        // SAFETY: The caller of `embedded()` guarantees the container
        // outlives this handle.
        unsafe { self.inner.as_ref() }
    }

    /// Acquires exclusive access to the guarded value.
    ///
    /// Returns a future that resolves to a [`EmbeddedLocalMutexGuard`]
    /// providing [`Deref`]/[`DerefMut`] access to the value.
    #[must_use]
    pub fn lock(&self) -> EmbeddedLocalMutexLockFuture<T> {
        EmbeddedLocalMutexLockFuture {
            inner: self.inner,
            awaiter: Awaiter::new(),
        }
    }

    /// Attempts to acquire the lock without blocking.
    #[must_use]
    // Mutating try_lock to always return None breaks tests.
    #[cfg_attr(test, mutants::skip)]
    pub fn try_lock(&self) -> Option<EmbeddedLocalMutexGuard<T>> {
        if self.inner().try_lock() {
            Some(EmbeddedLocalMutexGuard { inner: self.inner })
        } else {
            None
        }
    }
}

/// RAII guard returned by [`EmbeddedLocalMutexRef::lock()`] and
/// [`EmbeddedLocalMutexRef::try_lock()`].
///
/// Provides [`Deref`] and [`DerefMut`] access to the mutex-protected
/// value. The lock is released when the guard is dropped.
pub struct EmbeddedLocalMutexGuard<T> {
    inner: NonNull<Inner<T>>,
}

impl<T> Deref for EmbeddedLocalMutexGuard<T> {
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

impl<T> DerefMut for EmbeddedLocalMutexGuard<T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: The embedded() contract guarantees the container
        // outlives this guard.
        let inner = unsafe { self.inner.as_ref() };
        // SAFETY: We hold the lock and have &mut self, so exclusive
        // access to data is guaranteed.
        unsafe { &mut *inner.data.get() }
    }
}

impl<T> Drop for EmbeddedLocalMutexGuard<T> {
    // Mutating drop to a no-op would cause the lock to never release.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        // SAFETY: The embedded() contract guarantees the container
        // outlives this guard.
        let inner = unsafe { self.inner.as_ref() };
        inner.unlock();
    }
}

/// Future returned by [`EmbeddedLocalMutexRef::lock()`].
///
/// Completes with a [`EmbeddedLocalMutexGuard`] when the lock is acquired.
pub struct EmbeddedLocalMutexLockFuture<T> {
    inner: NonNull<Inner<T>>,

    awaiter: Awaiter,
}

impl<T> Future for EmbeddedLocalMutexLockFuture<T> {
    type Output = EmbeddedLocalMutexGuard<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<EmbeddedLocalMutexGuard<T>> {
        let waker = cx.waker().clone();

        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };

        // SAFETY: The container outlives this future per the
        // embedded() contract.
        let inner = unsafe { this.inner.as_ref() };
        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut this.awaiter) };
        // SAFETY: The lock_state is the lock this awaiter registers with.
        match unsafe { inner.poll_lock(awaiter, waker) } {
            Poll::Ready(()) => Poll::Ready(EmbeddedLocalMutexGuard { inner: this.inner }),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T> Drop for EmbeddedLocalMutexLockFuture<T> {
    // Inverting the is_registered() guard causes the Drop to hang
    // because it runs cleanup on an unregistered awaiter.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        // SAFETY: The container outlives this future per the
        // embedded() contract.
        let inner = unsafe { self.inner.as_ref() };
        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut self.awaiter) };
        // SAFETY: The lock_state is the lock this awaiter was
        // registered with.
        unsafe {
            inner.drop_lock_wait(awaiter);
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl<T> fmt::Debug for EmbeddedLocalMutex<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedLocalMutex").finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl<T> fmt::Debug for EmbeddedLocalMutexRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedLocalMutexRef")
            .finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl<T> fmt::Debug for EmbeddedLocalMutexGuard<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedLocalMutexGuard")
            .finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl<T> fmt::Debug for EmbeddedLocalMutexLockFuture<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedLocalMutexLockFuture")
            // SAFETY: Debug output is best-effort; no concurrent
            // mutation during formatting.
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {

    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(LocalMutex<u32>: Clone);
    assert_not_impl_any!(LocalMutex<u32>: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(
        LocalMutexGuard<'static, u32>: Send, Sync, Clone, UnwindSafe, RefUnwindSafe
    );
    assert_not_impl_any!(
        LocalMutexLockFuture<'static, u32>: Send, Sync, Unpin, UnwindSafe, RefUnwindSafe
    );

    // EmbeddedLocalMutex owns its UnsafeCell directly, so it is UnwindSafe by auto-trait.
    // Only RefUnwindSafe is absent (UnsafeCell is never RefUnwindSafe).
    assert_not_impl_any!(EmbeddedLocalMutex<u32>: Send, Sync, Unpin, RefUnwindSafe);
    assert_impl_all!(EmbeddedLocalMutexRef<u32>: Clone, Copy);
    assert_not_impl_any!(EmbeddedLocalMutexRef<u32>: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(
        EmbeddedLocalMutexGuard<u32>: Send, Sync, Clone, UnwindSafe, RefUnwindSafe
    );
    assert_not_impl_any!(
        EmbeddedLocalMutexLockFuture<u32>: Send, Sync, Unpin, UnwindSafe, RefUnwindSafe
    );

    #[test]
    fn starts_unlocked() {
        let mutex = LocalMutex::boxed(42_u32);
        let guard = mutex.try_lock();
        assert!(guard.is_some());
        assert_eq!(*guard.unwrap(), 42);
    }

    #[test]
    fn embedded_default_uses_value_default() {
        let container = Box::pin(EmbeddedLocalMutex::<u32>::default());
        // SAFETY: The container outlives all handles.
        let mutex = unsafe { LocalMutex::embedded(container.as_ref()) };
        let guard = mutex.try_lock();
        assert!(guard.is_some());
        assert_eq!(*guard.unwrap(), 0);
    }

    #[test]
    fn try_lock_when_locked_returns_none() {
        let mutex = LocalMutex::boxed(42_u32);
        let _guard = mutex.try_lock().unwrap();
        assert!(mutex.try_lock().is_none());
    }

    #[test]
    fn unlock_allows_try_lock() {
        let mutex = LocalMutex::boxed(42_u32);
        {
            let _guard = mutex.try_lock().unwrap();
        }
        assert!(mutex.try_lock().is_some());
    }

    #[test]
    fn clone_shares_state() {
        let a = LocalMutex::boxed(42_u32);
        let b = a.clone();
        let guard = a.try_lock().unwrap();
        assert!(b.try_lock().is_none());
        drop(guard);
        assert_eq!(*b.try_lock().unwrap(), 42);
    }

    #[test]
    fn guard_deref_mut() {
        let mutex = LocalMutex::boxed(0_u32);
        {
            let mut guard = mutex.try_lock().unwrap();
            *guard = 99;
        }
        assert_eq!(*mutex.try_lock().unwrap(), 99);
    }

    #[test]
    fn lock_completes_when_unlocked() {
        futures::executor::block_on(async {
            let mutex = LocalMutex::boxed(42_u32);
            let guard = mutex.lock().await;
            assert_eq!(*guard, 42);
        });
    }

    #[test]
    fn lock_completes_after_unlock() {
        let mutex = LocalMutex::boxed(42_u32);
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
        let mutex = LocalMutex::boxed(0_u32);
        let guard = mutex.try_lock().unwrap();

        let mut f1 = Box::pin(mutex.lock());
        let mut f2 = Box::pin(mutex.lock());
        let mut f3 = Box::pin(mutex.lock());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());
        assert!(f3.as_mut().poll(&mut cx).is_pending());

        drop(guard);
        let Poll::Ready(g1) = f1.as_mut().poll(&mut cx) else {
            panic!("expected Ready for f1")
        };
        assert!(f2.as_mut().poll(&mut cx).is_pending());
        assert!(f3.as_mut().poll(&mut cx).is_pending());

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
        let mutex = LocalMutex::boxed(0_u32);
        let guard = mutex.try_lock().unwrap();

        let mut f1 = Box::pin(mutex.lock());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());
        drop(guard);
        drop(f1);

        assert!(mutex.try_lock().is_some());
    }

    #[test]
    fn notified_then_dropped_forwards_to_next() {
        let mutex = LocalMutex::boxed(0_u32);
        let guard = mutex.try_lock().unwrap();

        let mut f1 = Box::pin(mutex.lock());
        let mut f2 = Box::pin(mutex.lock());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());

        drop(guard);
        drop(f1);

        assert!(f2.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn drop_unpolled_future_is_safe() {
        let mutex = LocalMutex::boxed(42_u32);
        {
            let _future = mutex.lock();
        }
        assert!(mutex.try_lock().is_some());
    }

    #[test]
    fn try_lock_fails_when_lock_transferred() {
        let mutex = LocalMutex::boxed(0_u32);
        let guard = mutex.try_lock().unwrap();

        let mut f1 = Box::pin(mutex.lock());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());

        // Drop the guard — lock ownership transfers to f1 (notified,
        // locked stays true). try_lock must fail because the lock is
        // still logically held.
        drop(guard);
        assert!(mutex.try_lock().is_none());

        // Confirm f1 can complete.
        assert!(f1.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn unlock_releases_on_panic() {
        let mutex = LocalMutex::boxed(0_u32);
        let handle = mutex.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _guard = handle.try_lock().unwrap();
            panic!("intentional");
        }));
        assert!(result.is_err());
        // Guard drop during unwinding must release the lock.
        assert!(mutex.try_lock().is_some());
    }

    #[test]
    fn multiple_sequential_cancellations() {
        let mutex = LocalMutex::boxed(0_u32);
        let guard = mutex.try_lock().unwrap();

        let mut f1 = Box::pin(mutex.lock());
        let mut f2 = Box::pin(mutex.lock());
        let mut f3 = Box::pin(mutex.lock());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());
        assert!(f3.as_mut().poll(&mut cx).is_pending());

        // Dropping the guard notifies f1.
        drop(guard);
        // Cancel f1 without polling — forwards to f2.
        drop(f1);
        // Cancel f2 without polling — forwards to f3.
        drop(f2);
        // f3 should now hold the lock.
        assert!(f3.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn embedded_lock_and_unlock() {
        futures::executor::block_on(async {
            let container = Box::pin(EmbeddedLocalMutex::new(42_u32));
            // SAFETY: The container outlives all handles.
            let mutex = unsafe { LocalMutex::embedded(container.as_ref()) };

            let guard = mutex.lock().await;
            assert_eq!(*guard, 42);
        });
    }

    #[test]
    fn embedded_clone_shares_state() {
        let container = Box::pin(EmbeddedLocalMutex::new(42_u32));
        // SAFETY: The container outlives all handles.
        let a = unsafe { LocalMutex::embedded(container.as_ref()) };
        let b = a;

        let guard = a.try_lock().unwrap();
        assert!(b.try_lock().is_none());
        drop(guard);
        assert_eq!(*b.try_lock().unwrap(), 42);
    }

    #[test]
    fn embedded_guard_deref_mut() {
        futures::executor::block_on(async {
            let container = Box::pin(EmbeddedLocalMutex::new(0_u32));
            // SAFETY: The container outlives all handles.
            let mutex = unsafe { LocalMutex::embedded(container.as_ref()) };

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
            let container = Box::pin(EmbeddedLocalMutex::new(42_u32));
            // SAFETY: The container outlives all handles.
            let mutex = unsafe { LocalMutex::embedded(container.as_ref()) };

            {
                let _future = mutex.lock();
            }
            let guard = mutex.lock().await;
            assert_eq!(*guard, 42);
        });
    }

    #[test]
    fn embedded_drop_registered_future() {
        let container = Box::pin(EmbeddedLocalMutex::new(42_u32));
        // SAFETY: The container outlives all handles.
        let mutex = unsafe { LocalMutex::embedded(container.as_ref()) };

        let guard = mutex.try_lock().unwrap();

        // Poll to register as a waiter (registered = true).
        let mut future = Box::pin(mutex.lock());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(future.as_mut().poll(&mut cx).is_pending());

        // Drop the registered future — must clean up the waiter node.
        drop(future);
        drop(guard);

        assert!(mutex.try_lock().is_some());
    }

    #[test]
    fn embedded_notified_then_dropped_forwards_to_next() {
        let container = Box::pin(EmbeddedLocalMutex::new(0_u32));
        // SAFETY: The container outlives all handles.
        let mutex = unsafe { LocalMutex::embedded(container.as_ref()) };

        let guard = mutex.try_lock().unwrap();

        let mut f1 = Box::pin(mutex.lock());
        let mut f2 = Box::pin(mutex.lock());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());

        // Drop guard — f1 is notified (lock transferred).
        drop(guard);
        // Drop f1 without polling — must forward to f2.
        drop(f1);

        assert!(f2.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn reentrant_waker_does_not_alias() {
        use testing::ReentrantWakerData;

        let mutex = LocalMutex::boxed(0_u32);
        let mutex_for_waker = mutex.clone();

        let waker_data = ReentrantWakerData::new(move || {
            // Re-entrantly try to lock the same mutex.
            // The lock was just transferred, so try_lock returns None.
            drop(mutex_for_waker.try_lock());
        });
        // SAFETY: Data outlives waker, single-threaded test.
        let waker = unsafe { waker_data.waker() };
        let mut cx = task::Context::from_waker(&waker);

        let guard = mutex.try_lock().unwrap();
        let mut future = Box::pin(mutex.lock());
        assert!(future.as_mut().poll(&mut cx).is_pending());

        // Drop the guard — transfers lock to the future, calling
        // the re-entrant waker which calls try_lock().
        drop(guard);

        assert!(waker_data.was_woken());
    }
}
