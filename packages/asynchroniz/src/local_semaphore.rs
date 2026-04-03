use std::cell::UnsafeCell;
use std::fmt;
use std::future::Future;
use std::marker::{PhantomData, PhantomPinned};
use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::Rc;
use std::task::{self, Poll, Waker};

use waiter_list::{WaiterList, WaiterSlot};

/// Single-threaded async semaphore.
///
/// This is the `!Send` counterpart of
/// [`Semaphore`][crate::Semaphore]. It avoids atomic operations and
/// locking, making it more efficient on single-threaded executors.
///
/// The semaphore is a lightweight cloneable handle. All clones
/// derived from the same [`boxed()`][Self::boxed] call share the same
/// underlying state.
///
/// # Examples
///
/// ```
/// use asynchroniz::LocalSemaphore;
///
/// #[tokio::main]
/// async fn main() {
///     let local = tokio::task::LocalSet::new();
///     local
///         .run_until(async {
///             let sem = LocalSemaphore::boxed(2);
///             let handle = sem.clone();
///
///             tokio::task::spawn_local(async move {
///                 let _permit = handle.acquire().await;
///             });
///
///             let _permit = sem.acquire().await;
///         })
///         .await;
/// }
/// ```
#[derive(Clone)]
pub struct LocalSemaphore {
    inner: Rc<Inner>,
}

struct SemaphoreState {
    available: usize,
    waiters: WaiterList,
}

struct Inner {
    state: UnsafeCell<SemaphoreState>,
    _not_send: PhantomData<*const ()>,
}

impl Inner {
    // Mutating release_permits to a no-op causes acquire futures to
    // hang.
    #[cfg_attr(test, mutants::skip)]
    fn release_permits(&self, count: usize) {
        // Combine the permit addition and the first waiter check,
        // avoiding a second state access when no waiters are queued.
        let waker = {
            // SAFETY: Single-threaded access guaranteed by !Send.
            let state = unsafe { &mut *self.state.get() };
            state.available = state
                .available
                .checked_add(count)
                .expect("permit count overflow is unreachable");
            Self::try_wake_head(state)
        };

        if let Some(w) = waker {
            w.wake();
            self.wake_waiters();
        }
    }

    /// Tries to satisfy the head waiter, returning its waker if
    /// successful.
    fn try_wake_head(state: &mut SemaphoreState) -> Option<Waker> {
        let head = state.waiters.head();

        if head.is_null() {
            return None;
        }

        // SAFETY: Single-threaded, head is non-null.
        let requested = unsafe { (*head).user_data() };

        if state.available >= requested {
            state.available = state
                .available
                .checked_sub(requested)
                .expect("available >= requested was just checked");

            // SAFETY: Single-threaded.
            let node =
                unsafe { state.waiters.pop_front() }.expect("head was non-null so pop cannot fail");
            // SAFETY: Single-threaded.
            unsafe {
                (*node).set_notified();
            }
            // SAFETY: Single-threaded.
            unsafe { (*node).take_waker() }
        } else {
            // Head-of-line blocking.
            None
        }
    }

    // Mutating wake_waiters to a no-op causes acquire futures to
    // hang.
    #[cfg_attr(test, mutants::skip)]
    fn wake_waiters(&self) {
        loop {
            let waker = {
                // SAFETY: Single-threaded access.
                let state = unsafe { &mut *self.state.get() };
                Self::try_wake_head(state)
            };

            if let Some(w) = waker {
                w.wake();
            } else {
                break;
            }
        }
    }

    // Mutating try_acquire to always return false breaks tests.
    #[cfg_attr(test, mutants::skip)]
    fn try_acquire(&self, permits: usize) -> bool {
        // SAFETY: Single-threaded access.
        let state = unsafe { &mut *self.state.get() };
        if state.available >= permits {
            state.available = state
                .available
                .checked_sub(permits)
                .expect("available >= permits was just checked");
            true
        } else {
            false
        }
    }

    /// # Safety
    ///
    /// * The `slot` must be pinned and must remain at the same memory
    ///   address for the lifetime of the acquire future.
    /// * The `slot` must belong to a future created from the same
    ///   semaphore.
    unsafe fn poll_acquire(&self, slot: &mut WaiterSlot, permits: usize, waker: Waker) -> Poll<()> {
        // SAFETY: Single-threaded access.
        if unsafe { slot.take_notification() } {
            return Poll::Ready(());
        }

        // SAFETY: Single-threaded access.
        let state = unsafe { &mut *self.state.get() };

        if !slot.is_registered() && state.available >= permits {
            state.available = state
                .available
                .checked_sub(permits)
                .expect("available >= permits was just checked");
            Poll::Ready(())
        } else {
            // SAFETY: Single-threaded, slot is pinned and not yet
            // in the list (or already registered with a stale waker).
            unsafe {
                slot.register_with_data(&mut state.waiters, waker, permits);
            }
            Poll::Pending
        }
    }

    /// # Safety
    ///
    /// Same requirements as [`poll_acquire`][Self::poll_acquire].
    unsafe fn drop_acquire_wait(&self, slot: &mut WaiterSlot, permits: usize) {
        let node_ptr = slot.node_ptr();

        // SAFETY: Single-threaded access.
        if unsafe { slot.is_notified() } {
            // We were given permits but the future was cancelled.
            // Return the permits and try to wake the head waiter in
            // the same access scope.
            let waker = {
                // SAFETY: Single-threaded access.
                let state = unsafe { &mut *self.state.get() };
                state.available = state
                    .available
                    .checked_add(permits)
                    .expect("permit count overflow is unreachable");
                Self::try_wake_head(state)
            };
            if let Some(w) = waker {
                w.wake();
                self.wake_waiters();
            }
        } else {
            // Not notified — just remove from the list.
            // SAFETY: Single-threaded access.
            let state = unsafe { &mut *self.state.get() };
            // SAFETY: Single-threaded, node is in the list.
            unsafe {
                state.waiters.remove(node_ptr);
            }
        }
    }
}

impl LocalSemaphore {
    /// Creates a new semaphore with the given number of permits.
    ///
    /// The state is heap-allocated. Clone the handle to share the
    /// same semaphore. For caller-provided storage, see
    /// [`embedded()`][Self::embedded].
    ///
    /// # Examples
    ///
    /// ```
    /// use asynchroniz::LocalSemaphore;
    ///
    /// let sem = LocalSemaphore::boxed(3);
    /// assert!(sem.try_acquire().is_some());
    /// ```
    #[must_use]
    pub fn boxed(permits: usize) -> Self {
        Self {
            inner: Rc::new(Inner {
                state: UnsafeCell::new(SemaphoreState {
                    available: permits,
                    waiters: WaiterList::new(),
                }),
                _not_send: PhantomData,
            }),
        }
    }

    /// Creates a handle from an [`EmbeddedLocalSemaphore`] container,
    /// avoiding heap allocation.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the [`EmbeddedLocalSemaphore`]
    /// outlives all returned handles, all
    /// [`RawLocalSemaphoreAcquireFuture`]s, and all
    /// [`RawLocalSemaphorePermit`]s created from them.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::pin::pin;
    ///
    /// use asynchroniz::{EmbeddedLocalSemaphore, LocalSemaphore};
    ///
    /// # futures::executor::block_on(async {
    /// let container = pin!(EmbeddedLocalSemaphore::new(1));
    ///
    /// // SAFETY: The container outlives the handle and all permits.
    /// let sem = unsafe { LocalSemaphore::embedded(container.as_ref()) };
    ///
    /// let permit = sem.acquire().await;
    /// assert!(sem.try_acquire().is_none());
    /// drop(permit);
    /// assert!(sem.try_acquire().is_some());
    /// # });
    /// ```
    #[must_use]
    pub unsafe fn embedded(place: Pin<&EmbeddedLocalSemaphore>) -> RawLocalSemaphore {
        let inner = NonNull::from(&place.get_ref().inner);
        RawLocalSemaphore { inner }
    }

    /// Returns a future that resolves to a [`LocalSemaphorePermit`]
    /// when a single permit is available.
    ///
    /// # Examples
    ///
    /// ```
    /// use asynchroniz::LocalSemaphore;
    ///
    /// # futures::executor::block_on(async {
    /// let sem = LocalSemaphore::boxed(1);
    /// let permit = sem.acquire().await;
    /// assert!(sem.try_acquire().is_none());
    /// drop(permit);
    /// # });
    /// ```
    #[must_use]
    pub fn acquire(&self) -> LocalSemaphoreAcquireFuture<'_> {
        self.acquire_many(1)
    }

    /// Returns a future that resolves to a [`LocalSemaphorePermit`]
    /// holding `permits` permits.
    ///
    /// # Panics
    ///
    /// Panics if `permits` is zero.
    #[must_use]
    pub fn acquire_many(&self, permits: usize) -> LocalSemaphoreAcquireFuture<'_> {
        assert!(permits > 0, "cannot acquire zero permits");

        LocalSemaphoreAcquireFuture {
            inner: &self.inner,
            permits,
            slot: WaiterSlot::new(),
        }
    }

    /// Attempts to acquire a single permit without blocking.
    ///
    /// # Examples
    ///
    /// ```
    /// use asynchroniz::LocalSemaphore;
    ///
    /// let sem = LocalSemaphore::boxed(1);
    /// let permit = sem.try_acquire().unwrap();
    /// assert!(sem.try_acquire().is_none());
    /// ```
    #[must_use]
    // Mutating try_acquire to always return None breaks tests.
    #[cfg_attr(test, mutants::skip)]
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn try_acquire(&self) -> Option<LocalSemaphorePermit<'_>> {
        self.try_acquire_many(1)
    }

    /// Attempts to acquire `permits` permits without blocking.
    ///
    /// # Panics
    ///
    /// Panics if `permits` is zero.
    #[must_use]
    // Mutating try_acquire_many to always return None breaks tests.
    #[cfg_attr(test, mutants::skip)]
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn try_acquire_many(&self, permits: usize) -> Option<LocalSemaphorePermit<'_>> {
        assert!(permits > 0, "cannot acquire zero permits");

        if self.inner.try_acquire(permits) {
            Some(LocalSemaphorePermit {
                inner: &self.inner,
                permits,
            })
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// LocalSemaphorePermit
// ---------------------------------------------------------------------------

/// RAII permit returned by [`LocalSemaphore::acquire()`] and
/// [`LocalSemaphore::try_acquire()`].
///
/// The permit is returned to the semaphore when dropped.
pub struct LocalSemaphorePermit<'a> {
    inner: &'a Inner,
    permits: usize,
}

impl Drop for LocalSemaphorePermit<'_> {
    // Mutating drop to a no-op causes permits to leak.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        self.inner.release_permits(self.permits);
    }
}

// ---------------------------------------------------------------------------
// LocalSemaphoreAcquireFuture
// ---------------------------------------------------------------------------

/// Future returned by [`LocalSemaphore::acquire()`] and
/// [`LocalSemaphore::acquire_many()`].
///
/// Completes with a [`LocalSemaphorePermit`] when enough permits are
/// available.
pub struct LocalSemaphoreAcquireFuture<'a> {
    inner: &'a Inner,
    permits: usize,

    slot: WaiterSlot,
}

impl<'a> Future for LocalSemaphoreAcquireFuture<'a> {
    type Output = LocalSemaphorePermit<'a>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<LocalSemaphorePermit<'a>> {
        let waker = cx.waker().clone();

        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };

        // SAFETY: The slot is pinned (via WaiterSlot's PhantomPinned)
        // and the state field is the lock this slot registers with.
        match unsafe { this.inner.poll_acquire(&mut this.slot, this.permits, waker) } {
            Poll::Ready(()) => Poll::Ready(LocalSemaphorePermit {
                inner: this.inner,
                permits: this.permits,
            }),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for LocalSemaphoreAcquireFuture<'_> {
    fn drop(&mut self) {
        if !self.slot.is_registered() {
            return;
        }

        // SAFETY: The slot is pinned (via WaiterSlot's PhantomPinned)
        // and the state is the lock this slot was registered with.
        unsafe {
            self.inner.drop_acquire_wait(&mut self.slot, self.permits);
        }
    }
}

// ---------------------------------------------------------------------------
// Debug impls
// ---------------------------------------------------------------------------

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for LocalSemaphore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalSemaphore").finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for LocalSemaphorePermit<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalSemaphorePermit")
            .field("permits", &self.permits)
            .finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for LocalSemaphoreAcquireFuture<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalSemaphoreAcquireFuture")
            .field("permits", &self.permits)
            .field("registered", &self.slot.is_registered())
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Embedded variant
// ---------------------------------------------------------------------------

/// Embedded-state container for [`LocalSemaphore`].
///
/// Stores the semaphore state inline, avoiding the heap allocation
/// that [`LocalSemaphore::boxed()`] requires.
///
/// # Examples
///
/// ```
/// use std::pin::pin;
///
/// use asynchroniz::{EmbeddedLocalSemaphore, LocalSemaphore};
///
/// # futures::executor::block_on(async {
/// let container = pin!(EmbeddedLocalSemaphore::new(1));
///
/// // SAFETY: The container outlives the handle and all permits.
/// let sem = unsafe { LocalSemaphore::embedded(container.as_ref()) };
///
/// let permit = sem.acquire().await;
/// assert!(sem.try_acquire().is_none());
/// # });
/// ```
pub struct EmbeddedLocalSemaphore {
    inner: Inner,
    _pinned: PhantomPinned,
}

impl EmbeddedLocalSemaphore {
    /// Creates a new embedded semaphore container with the given
    /// number of permits.
    #[must_use]
    pub fn new(permits: usize) -> Self {
        Self {
            inner: Inner {
                state: UnsafeCell::new(SemaphoreState {
                    available: permits,
                    waiters: WaiterList::new(),
                }),
                _not_send: PhantomData,
            },
            _pinned: PhantomPinned,
        }
    }
}

/// Handle to an embedded [`LocalSemaphore`].
///
/// Created via [`LocalSemaphore::embedded()`]. The caller is
/// responsible for ensuring the [`EmbeddedLocalSemaphore`] outlives
/// all handles, acquire futures, and permits.
///
/// The API is identical to [`LocalSemaphore`].
#[derive(Clone, Copy)]
pub struct RawLocalSemaphore {
    inner: NonNull<Inner>,
}

impl RawLocalSemaphore {
    fn inner(&self) -> &Inner {
        // SAFETY: The caller of `embedded()` guarantees the
        // container outlives this handle.
        unsafe { self.inner.as_ref() }
    }

    /// Returns a future that resolves to a
    /// [`RawLocalSemaphorePermit`] when a single permit is available.
    #[must_use]
    pub fn acquire(&self) -> RawLocalSemaphoreAcquireFuture {
        self.acquire_many(1)
    }

    /// Returns a future that resolves to a
    /// [`RawLocalSemaphorePermit`] holding `permits` permits.
    ///
    /// # Panics
    ///
    /// Panics if `permits` is zero.
    #[must_use]
    pub fn acquire_many(&self, permits: usize) -> RawLocalSemaphoreAcquireFuture {
        assert!(permits > 0, "cannot acquire zero permits");

        RawLocalSemaphoreAcquireFuture {
            inner: self.inner,
            permits,
            slot: WaiterSlot::new(),
        }
    }

    /// Attempts to acquire a single permit without blocking.
    #[must_use]
    // Mutating try_acquire to always return None breaks tests.
    #[cfg_attr(test, mutants::skip)]
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn try_acquire(&self) -> Option<RawLocalSemaphorePermit> {
        self.try_acquire_many(1)
    }

    /// Attempts to acquire `permits` permits without blocking.
    ///
    /// # Panics
    ///
    /// Panics if `permits` is zero.
    #[must_use]
    // Mutating try_acquire_many to always return None breaks tests.
    #[cfg_attr(test, mutants::skip)]
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn try_acquire_many(&self, permits: usize) -> Option<RawLocalSemaphorePermit> {
        assert!(permits > 0, "cannot acquire zero permits");

        if self.inner().try_acquire(permits) {
            Some(RawLocalSemaphorePermit {
                inner: self.inner,
                permits,
            })
        } else {
            None
        }
    }
}

/// RAII permit returned by [`RawLocalSemaphore::acquire()`] and
/// [`RawLocalSemaphore::try_acquire()`].
pub struct RawLocalSemaphorePermit {
    inner: NonNull<Inner>,
    permits: usize,
}

impl Drop for RawLocalSemaphorePermit {
    // Mutating drop to a no-op causes permits to leak.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        // SAFETY: The embedded() contract guarantees the container
        // outlives this permit.
        let inner = unsafe { self.inner.as_ref() };
        inner.release_permits(self.permits);
    }
}

/// Future returned by [`RawLocalSemaphore::acquire()`] and
/// [`RawLocalSemaphore::acquire_many()`].
pub struct RawLocalSemaphoreAcquireFuture {
    inner: NonNull<Inner>,
    permits: usize,

    slot: WaiterSlot,
}

impl Future for RawLocalSemaphoreAcquireFuture {
    type Output = RawLocalSemaphorePermit;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<RawLocalSemaphorePermit> {
        let waker = cx.waker().clone();

        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };

        // SAFETY: The container outlives this future per the
        // embedded() contract.
        let inner = unsafe { this.inner.as_ref() };
        // SAFETY: poll_acquire requires single-threaded access,
        // which LocalSemaphore guarantees (!Send).
        match unsafe { inner.poll_acquire(&mut this.slot, this.permits, waker) } {
            Poll::Ready(()) => Poll::Ready(RawLocalSemaphorePermit {
                inner: this.inner,
                permits: this.permits,
            }),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for RawLocalSemaphoreAcquireFuture {
    fn drop(&mut self) {
        if !self.slot.is_registered() {
            return;
        }

        // SAFETY: The container outlives this future per the
        // embedded() contract.
        let inner = unsafe { self.inner.as_ref() };
        // SAFETY: drop_acquire_wait requires single-threaded access,
        // which LocalSemaphore guarantees (!Send).
        unsafe {
            inner.drop_acquire_wait(&mut self.slot, self.permits);
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for EmbeddedLocalSemaphore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedLocalSemaphore")
            .finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for RawLocalSemaphore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawLocalSemaphore").finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for RawLocalSemaphorePermit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawLocalSemaphorePermit")
            .field("permits", &self.permits)
            .finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for RawLocalSemaphoreAcquireFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawLocalSemaphoreAcquireFuture")
            .field("permits", &self.permits)
            .field("registered", &self.slot.is_registered())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    // --- trait assertions ---

    assert_impl_all!(LocalSemaphore: Clone);
    assert_not_impl_any!(LocalSemaphore: Send, Sync);
    assert_not_impl_any!(LocalSemaphorePermit<'static>: Send, Sync, Clone);
    assert_not_impl_any!(LocalSemaphoreAcquireFuture<'static>: Send, Sync, Unpin);

    assert_not_impl_any!(EmbeddedLocalSemaphore: Send, Sync, Unpin);
    assert_impl_all!(RawLocalSemaphore: Clone, Copy);
    assert_not_impl_any!(RawLocalSemaphore: Send, Sync);
    assert_not_impl_any!(RawLocalSemaphorePermit: Send, Sync, Clone);
    assert_not_impl_any!(RawLocalSemaphoreAcquireFuture: Send, Sync, Unpin);

    // --- basic functionality ---

    #[test]
    fn acquire_and_release() {
        let sem = LocalSemaphore::boxed(1);
        let permit = sem.try_acquire().unwrap();
        assert!(sem.try_acquire().is_none());
        drop(permit);
        assert!(sem.try_acquire().is_some());
    }

    #[test]
    fn multiple_permits() {
        let sem = LocalSemaphore::boxed(3);
        let p1 = sem.try_acquire().unwrap();
        let p2 = sem.try_acquire().unwrap();
        let p3 = sem.try_acquire().unwrap();
        assert!(sem.try_acquire().is_none());
        drop(p1);
        assert!(sem.try_acquire().is_some());
        drop(p2);
        drop(p3);
    }

    #[test]
    fn try_acquire_many() {
        let sem = LocalSemaphore::boxed(5);
        let permit = sem.try_acquire_many(3).unwrap();
        assert!(sem.try_acquire_many(3).is_none());
        assert!(sem.try_acquire_many(2).is_some());
        drop(permit);
    }

    #[test]
    fn clone_shares_state() {
        let a = LocalSemaphore::boxed(1);
        let b = a.clone();
        let permit = a.try_acquire().unwrap();
        assert!(b.try_acquire().is_none());
        drop(permit);
        assert!(b.try_acquire().is_some());
    }

    #[test]
    #[should_panic]
    fn acquire_zero_panics() {
        let sem = LocalSemaphore::boxed(1);
        let _future = sem.acquire_many(0);
    }

    #[test]
    #[should_panic]
    fn try_acquire_zero_panics() {
        let sem = LocalSemaphore::boxed(1);
        let _permit = sem.try_acquire_many(0);
    }

    // --- async tests ---

    #[test]
    fn acquire_completes_when_available() {
        futures::executor::block_on(async {
            let sem = LocalSemaphore::boxed(1);
            let permit = sem.acquire().await;
            assert!(sem.try_acquire().is_none());
            drop(permit);
        });
    }

    #[test]
    fn acquire_completes_after_release() {
        let sem = LocalSemaphore::boxed(1);
        let permit = sem.try_acquire().unwrap();
        let mut future = Box::pin(sem.acquire());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(future.as_mut().poll(&mut cx).is_pending());
        drop(permit);
        assert!(future.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn fifo_ordering() {
        let sem = LocalSemaphore::boxed(1);
        let permit = sem.try_acquire().unwrap();

        let mut f1 = Box::pin(sem.acquire());
        let mut f2 = Box::pin(sem.acquire());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());

        drop(permit);
        let Poll::Ready(p1) = f1.as_mut().poll(&mut cx) else {
            panic!("expected Ready for f1")
        };
        assert!(f2.as_mut().poll(&mut cx).is_pending());

        drop(p1);
        assert!(f2.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn head_of_line_blocking() {
        let sem = LocalSemaphore::boxed(2);
        let _p1 = sem.try_acquire().unwrap();
        let _p2 = sem.try_acquire().unwrap();

        let mut f1 = Box::pin(sem.acquire_many(2));
        let mut f2 = Box::pin(sem.acquire());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());

        drop(_p1);
        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());

        drop(_p2);
        let Poll::Ready(p_f1) = f1.as_mut().poll(&mut cx) else {
            panic!("expected Ready for f1")
        };
        assert!(f2.as_mut().poll(&mut cx).is_pending());

        drop(p_f1);
        assert!(f2.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn cancelled_waiter_returns_permits() {
        let sem = LocalSemaphore::boxed(1);
        let permit = sem.try_acquire().unwrap();

        let mut f1 = Box::pin(sem.acquire());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());
        drop(permit);
        drop(f1);

        assert!(sem.try_acquire().is_some());
    }

    #[test]
    fn notified_then_dropped_wakes_next() {
        let sem = LocalSemaphore::boxed(1);
        let permit = sem.try_acquire().unwrap();

        let mut f1 = Box::pin(sem.acquire());
        let mut f2 = Box::pin(sem.acquire());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());

        drop(permit);
        drop(f1);

        assert!(f2.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn drop_unpolled_future_is_safe() {
        let sem = LocalSemaphore::boxed(1);
        {
            let _future = sem.acquire();
        }
        assert!(sem.try_acquire().is_some());
    }

    #[test]
    fn zero_initial_permits() {
        let sem = LocalSemaphore::boxed(0);
        assert!(sem.try_acquire().is_none());
    }

    #[test]
    fn try_acquire_many_exact_max() {
        let sem = LocalSemaphore::boxed(5);
        let permit = sem.try_acquire_many(5).unwrap();
        assert!(sem.try_acquire().is_none());
        drop(permit);
        assert!(sem.try_acquire().is_some());
    }

    #[test]
    fn try_acquire_many_exceeds_max() {
        let sem = LocalSemaphore::boxed(3);
        assert!(sem.try_acquire_many(4).is_none());
        // Semaphore is untouched — still has 3 available.
        assert!(sem.try_acquire_many(3).is_some());
    }

    #[test]
    fn acquire_many_all_at_once() {
        futures::executor::block_on(async {
            let sem = LocalSemaphore::boxed(5);
            let permit = sem.acquire_many(5).await;
            assert!(sem.try_acquire().is_none());
            drop(permit);
            assert!(sem.try_acquire_many(5).is_some());
        });
    }

    #[test]
    fn try_acquire_bypasses_waiter_queue() {
        let sem = LocalSemaphore::boxed(3);
        let _p1 = sem.try_acquire().unwrap();
        let _p2 = sem.try_acquire().unwrap();
        // 1 permit available, 2 held.

        // Register a waiter wanting 2 permits (more than available).
        let mut f1 = Box::pin(sem.acquire_many(2));
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());

        // try_acquire(1) succeeds despite a queued waiter because
        // try_acquire does not consult the waiter queue.
        assert!(sem.try_acquire().is_some());
    }

    #[test]
    fn release_wakes_multiple_waiters() {
        let sem = LocalSemaphore::boxed(3);
        let p1 = sem.try_acquire().unwrap();
        let p2 = sem.try_acquire().unwrap();
        let p3 = sem.try_acquire().unwrap();

        let mut f1 = Box::pin(sem.acquire());
        let mut f2 = Box::pin(sem.acquire());
        let mut f3 = Box::pin(sem.acquire());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());
        assert!(f3.as_mut().poll(&mut cx).is_pending());

        // Release all 3 permits. Each release wakes the next waiter.
        drop(p1);
        drop(p2);
        drop(p3);

        assert!(f1.as_mut().poll(&mut cx).is_ready());
        assert!(f2.as_mut().poll(&mut cx).is_ready());
        assert!(f3.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn multiple_sequential_cancellations() {
        let sem = LocalSemaphore::boxed(1);
        let permit = sem.try_acquire().unwrap();

        let mut f1 = Box::pin(sem.acquire());
        let mut f2 = Box::pin(sem.acquire());
        let mut f3 = Box::pin(sem.acquire());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());
        assert!(f3.as_mut().poll(&mut cx).is_pending());

        // Dropping the permit notifies f1.
        drop(permit);
        // Cancel f1 without polling — forwards to f2.
        drop(f1);
        // Cancel f2 without polling — forwards to f3.
        drop(f2);
        // f3 should now have the permit.
        assert!(f3.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn permits_released_on_panic() {
        let sem = LocalSemaphore::boxed(1);
        let handle = sem.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _permit = handle.try_acquire().unwrap();
            panic!("intentional");
        }));
        assert!(result.is_err());
        // Permit drop during unwinding must return the permit.
        assert!(sem.try_acquire().is_some());
    }

    // --- embedded variant tests ---

    #[test]
    fn embedded_acquire_and_release() {
        futures::executor::block_on(async {
            let container = Box::pin(EmbeddedLocalSemaphore::new(1));
            // SAFETY: The container outlives all handles.
            let sem = unsafe { LocalSemaphore::embedded(container.as_ref()) };

            let permit = sem.acquire().await;
            assert!(sem.try_acquire().is_none());
            drop(permit);
            assert!(sem.try_acquire().is_some());
        });
    }

    #[test]
    fn embedded_clone_shares_state() {
        let container = Box::pin(EmbeddedLocalSemaphore::new(1));
        // SAFETY: The container outlives all handles.
        let a = unsafe { LocalSemaphore::embedded(container.as_ref()) };
        let b = a;

        let permit = a.try_acquire().unwrap();
        assert!(b.try_acquire().is_none());
        drop(permit);
        assert!(b.try_acquire().is_some());
    }

    #[test]
    fn embedded_drop_future_while_waiting() {
        futures::executor::block_on(async {
            let container = Box::pin(EmbeddedLocalSemaphore::new(1));
            // SAFETY: The container outlives all handles.
            let sem = unsafe { LocalSemaphore::embedded(container.as_ref()) };

            {
                let _future = sem.acquire();
            }
            let permit = sem.acquire().await;
            assert!(sem.try_acquire().is_none());
            drop(permit);
        });
    }

    #[test]
    fn embedded_drop_registered_future() {
        let container = Box::pin(EmbeddedLocalSemaphore::new(1));
        // SAFETY: The container outlives all handles.
        let sem = unsafe { LocalSemaphore::embedded(container.as_ref()) };

        let permit = sem.try_acquire().unwrap();

        // Poll to register as a waiter (registered = true).
        let mut future = Box::pin(sem.acquire());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(future.as_mut().poll(&mut cx).is_pending());

        // Drop the registered future — must clean up the waiter node.
        drop(future);
        drop(permit);

        assert!(sem.try_acquire().is_some());
    }

    #[test]
    fn embedded_notified_then_dropped_forwards_to_next() {
        let container = Box::pin(EmbeddedLocalSemaphore::new(1));
        // SAFETY: The container outlives all handles.
        let sem = unsafe { LocalSemaphore::embedded(container.as_ref()) };

        let permit = sem.try_acquire().unwrap();

        let mut f1 = Box::pin(sem.acquire());
        let mut f2 = Box::pin(sem.acquire());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());

        // Drop permit — f1 is notified.
        drop(permit);
        // Drop f1 without polling — must forward to f2.
        drop(f1);

        assert!(f2.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn multi_permit_release_wakes_multiple_single_permit_waiters() {
        // Releasing a multi-permit hold should wake multiple
        // single-permit waiters via the wake_waiters loop.
        let sem = LocalSemaphore::boxed(2);
        let big_permit = sem.try_acquire_many(2).unwrap();

        let mut f1 = Box::pin(sem.acquire());
        let mut f2 = Box::pin(sem.acquire());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());

        // Release 2 permits at once — try_wake_head handles the
        // first waiter, wake_waiters must find the second.
        drop(big_permit);

        assert!(f1.as_mut().poll(&mut cx).is_ready());
        assert!(f2.as_mut().poll(&mut cx).is_ready());
    }
}
