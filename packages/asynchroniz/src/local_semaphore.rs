use std::cell::UnsafeCell;
use std::fmt;
use std::future::Future;
use std::marker::{PhantomData, PhantomPinned};
use std::num::NonZero;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::Rc;
use std::task::{self, Poll, Waker};

use awaiter_set::{Awaiter, AwaiterSet};

use crate::constants::ONE_PERMIT;

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
/// To avoid the heap allocation, use [`EmbeddedLocalSemaphore`] with
/// [`embedded()`][Self::embedded] instead.
///
/// # Fairness
///
/// Waiters are served in FIFO order with head-of-line blocking: if
/// the first waiter requests more permits than are currently
/// available, later waiters also wait even if they could be satisfied.
/// This prevents starvation of multi-permit acquires.
///
/// # Cancellation safety
///
/// If an acquire future that has been notified is dropped before it
/// is polled to completion, the permits are returned and any newly
/// satisfiable waiters are woken. No permits are lost due to
/// cancellation.
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
///
///             tokio::task::spawn_local({
///                 let sem = sem.clone();
///                 async move {
///                     let _permit = sem.acquire().await;
///                 }
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
    waiters: AwaiterSet,
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
            Self::try_wake_one(state)
        };

        if let Some(w) = waker {
            w.wake();
            self.wake_waiters();
        }
    }

    /// Finds the first waiter whose permit request can be satisfied,
    /// deducts the permits, removes it from the set and returns its
    /// waker.
    // Mutating try_wake_one to return None causes acquire futures
    // to hang because waiters are never woken despite available
    // permits.
    #[cfg_attr(test, mutants::skip)]
    fn try_wake_one(state: &mut SemaphoreState) -> Option<Waker> {
        let available = &mut state.available;
        let predicate = |awaiter: &Awaiter| {
            // SAFETY: Single-threaded access.
            let requested = unsafe { awaiter.user_data() };
            if requested <= *available {
                *available = available
                    .checked_sub(requested)
                    .expect("available >= requested was just checked");
                true
            } else {
                false
            }
        };
        // SAFETY: Single-threaded access.
        unsafe { state.waiters.notify_one_if(predicate) }
    }

    /// Wakes queued waiters whose permit requests can be satisfied.
    // Mutating wake_waiters to a no-op causes acquire futures to
    // hang.
    #[cfg_attr(test, mutants::skip)]
    fn wake_waiters(&self) {
        loop {
            let waker = {
                // SAFETY: Single-threaded access.
                let state = unsafe { &mut *self.state.get() };
                Self::try_wake_one(state)
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
    /// * The `awaiter` must belong to a future created from the same
    ///   semaphore.
    unsafe fn poll_acquire(
        &self,
        mut awaiter: Pin<&mut Awaiter>,
        permits: usize,
        waker: Waker,
    ) -> Poll<()> {
        // SAFETY: Single-threaded access.
        if unsafe { awaiter.as_mut().take_notification() } {
            return Poll::Ready(());
        }

        // SAFETY: Single-threaded access.
        let state = unsafe { &mut *self.state.get() };

        // SAFETY: Single-threaded access.
        if !unsafe { awaiter.is_registered() }
            // SAFETY: Single-threaded access.
            && unsafe { state.waiters.is_empty() }
            && state.available >= permits
        {
            state.available = state
                .available
                .checked_sub(permits)
                .expect("available >= permits was just checked");
            Poll::Ready(())
        } else {
            // Register or update the waker.
            // SAFETY: Single-threaded, awaiter is pinned and not yet
            // in the set (or already registered for waker update).
            unsafe {
                state
                    .waiters
                    .register_with_data(awaiter.as_mut(), waker, permits);
            }
            Poll::Pending
        }
    }

    /// # Safety
    ///
    /// Same requirements as [`poll_acquire`][Self::poll_acquire].
    unsafe fn drop_acquire_wait(&self, mut awaiter: Pin<&mut Awaiter>, permits: usize) {
        // SAFETY: Single-threaded access.
        if !unsafe { awaiter.is_registered() } {
            return;
        }

        // SAFETY: Single-threaded access.
        if unsafe { awaiter.as_ref().is_notified() } {
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
                Self::try_wake_one(state)
            };
            if let Some(w) = waker {
                w.wake();
                self.wake_waiters();
            }
        } else {
            // Not notified — just remove from the set.
            // SAFETY: Single-threaded access.
            let state = unsafe { &mut *self.state.get() };
            // SAFETY: Single-threaded, node is in the set.
            unsafe {
                state.waiters.unregister(awaiter.as_mut());
            }
        }
    }
}

// All state access is confined to a single thread (!Send). The
// UnsafeCell contents are only accessed through methods with
// serialized-access contracts. No inconsistent state can be observed
// during unwind.
impl UnwindSafe for LocalSemaphore {}
impl RefUnwindSafe for LocalSemaphore {}

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
                    waiters: AwaiterSet::new(),
                }),
                _not_send: PhantomData,
            }),
        }
    }

    /// Creates an instance that references the state in the
    /// [`EmbeddedLocalSemaphore`].
    ///
    /// # Safety
    ///
    /// The caller must ensure that the [`EmbeddedLocalSemaphore`]
    /// outlives all returned handles, all
    /// [`EmbeddedLocalSemaphoreAcquireFuture`]s, and all
    /// [`EmbeddedLocalSemaphorePermit`]s created from them.
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
    pub unsafe fn embedded(place: Pin<&EmbeddedLocalSemaphore>) -> EmbeddedLocalSemaphoreRef {
        let inner = NonNull::from(&place.get_ref().inner);
        EmbeddedLocalSemaphoreRef { inner }
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
        self.acquire_many(ONE_PERMIT)
    }

    /// Returns a future that resolves to a [`LocalSemaphorePermit`]
    /// holding `permits` permits.
    #[must_use]
    pub fn acquire_many(&self, permits: NonZero<usize>) -> LocalSemaphoreAcquireFuture<'_> {
        LocalSemaphoreAcquireFuture {
            inner: &self.inner,
            permits: permits.get(),
            awaiter: Awaiter::new(),
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
    pub fn try_acquire(&self) -> Option<LocalSemaphorePermit<'_>> {
        self.try_acquire_many(ONE_PERMIT)
    }

    /// Attempts to acquire `permits` permits without blocking.
    #[must_use]
    // Mutating try_acquire_many to always return None breaks tests.
    #[cfg_attr(test, mutants::skip)]
    pub fn try_acquire_many(&self, permits: NonZero<usize>) -> Option<LocalSemaphorePermit<'_>> {
        let permits = permits.get();

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

// All state access is confined to a single thread (!Send). The
// UnsafeCell contents are only accessed through methods with
// serialized-access contracts. No inconsistent state can be observed
// during unwind.
impl UnwindSafe for LocalSemaphorePermit<'_> {}
impl RefUnwindSafe for LocalSemaphorePermit<'_> {}

/// Future returned by [`LocalSemaphore::acquire()`] and
/// [`LocalSemaphore::acquire_many()`].
///
/// Completes with a [`LocalSemaphorePermit`] when enough permits are
/// available.
pub struct LocalSemaphoreAcquireFuture<'a> {
    inner: &'a Inner,
    permits: usize,

    awaiter: Awaiter,
}

// All state access is confined to a single thread (!Send). The
// UnsafeCell contents are only accessed through methods with
// serialized-access contracts. No inconsistent state can be observed
// during unwind.
impl UnwindSafe for LocalSemaphoreAcquireFuture<'_> {}
impl RefUnwindSafe for LocalSemaphoreAcquireFuture<'_> {}

impl<'a> Future for LocalSemaphoreAcquireFuture<'a> {
    type Output = LocalSemaphorePermit<'a>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<LocalSemaphorePermit<'a>> {
        let waker = cx.waker().clone();

        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };

        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut this.awaiter) };
        // SAFETY: The state field is the semaphore state this awaiter registers
        // with.
        match unsafe { this.inner.poll_acquire(awaiter, this.permits, waker) } {
            Poll::Ready(()) => Poll::Ready(LocalSemaphorePermit {
                inner: this.inner,
                permits: this.permits,
            }),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for LocalSemaphoreAcquireFuture<'_> {
    // Inverting the is_registered() guard causes the Drop to hang
    // because it runs cleanup on an unregistered awaiter.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut self.awaiter) };
        // SAFETY: The state is the lock this awaiter was registered
        // with.
        unsafe {
            self.inner.drop_acquire_wait(awaiter, self.permits);
        }
    }
}

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
            // SAFETY: Debug output is best-effort; no concurrent
            // mutation during formatting.
            .finish_non_exhaustive()
    }
}

/// Inline storage for semaphore state, avoiding heap allocation.
///
/// Pin the container, then call [`LocalSemaphore::embedded()`] to
/// obtain a [`EmbeddedLocalSemaphoreRef`] reference that operates on the
/// embedded state.
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

// All state access is confined to a single thread (!Send). The
// UnsafeCell contents are only accessed through methods with
// serialized-access contracts. No inconsistent state can be observed
// during unwind.
impl UnwindSafe for EmbeddedLocalSemaphore {}
impl RefUnwindSafe for EmbeddedLocalSemaphore {}

impl EmbeddedLocalSemaphore {
    /// Creates a new embedded semaphore container with the given
    /// number of permits.
    #[must_use]
    pub fn new(permits: usize) -> Self {
        Self {
            inner: Inner {
                state: UnsafeCell::new(SemaphoreState {
                    available: permits,
                    waiters: AwaiterSet::new(),
                }),
                _not_send: PhantomData,
            },
            _pinned: PhantomPinned,
        }
    }
}

/// Reference to an [`EmbeddedLocalSemaphore`].
///
/// Created via [`LocalSemaphore::embedded()`]. The caller is
/// responsible for ensuring the [`EmbeddedLocalSemaphore`] outlives
/// all handles, acquire futures, and permits.
///
/// The API is identical to [`LocalSemaphore`].
#[derive(Clone, Copy)]
pub struct EmbeddedLocalSemaphoreRef {
    inner: NonNull<Inner>,
}

// The NonNull pointer is only dereferenced on the owning thread.
// All state access is confined to a single thread (!Send). No
// inconsistent state can be observed during unwind.
impl UnwindSafe for EmbeddedLocalSemaphoreRef {}
impl RefUnwindSafe for EmbeddedLocalSemaphoreRef {}

impl EmbeddedLocalSemaphoreRef {
    fn inner(&self) -> &Inner {
        // SAFETY: The caller of `embedded()` guarantees the
        // container outlives this handle.
        unsafe { self.inner.as_ref() }
    }

    /// Returns a future that resolves to a
    /// [`EmbeddedLocalSemaphorePermit`] when a single permit is available.
    #[must_use]
    pub fn acquire(&self) -> EmbeddedLocalSemaphoreAcquireFuture {
        self.acquire_many(ONE_PERMIT)
    }

    /// Returns a future that resolves to a
    /// [`EmbeddedLocalSemaphorePermit`] holding `permits` permits.
    #[must_use]
    pub fn acquire_many(&self, permits: NonZero<usize>) -> EmbeddedLocalSemaphoreAcquireFuture {
        EmbeddedLocalSemaphoreAcquireFuture {
            inner: self.inner,
            permits: permits.get(),
            awaiter: Awaiter::new(),
        }
    }

    /// Attempts to acquire a single permit without blocking.
    #[must_use]
    // Mutating try_acquire to always return None breaks tests.
    #[cfg_attr(test, mutants::skip)]
    pub fn try_acquire(&self) -> Option<EmbeddedLocalSemaphorePermit> {
        self.try_acquire_many(ONE_PERMIT)
    }

    /// Attempts to acquire `permits` permits without blocking.
    #[must_use]
    // Mutating try_acquire_many to always return None breaks tests.
    #[cfg_attr(test, mutants::skip)]
    pub fn try_acquire_many(
        &self,
        permits: NonZero<usize>,
    ) -> Option<EmbeddedLocalSemaphorePermit> {
        let permits = permits.get();

        if self.inner().try_acquire(permits) {
            Some(EmbeddedLocalSemaphorePermit {
                inner: self.inner,
                permits,
            })
        } else {
            None
        }
    }
}

/// RAII permit returned by [`EmbeddedLocalSemaphoreRef::acquire()`] and
/// [`EmbeddedLocalSemaphoreRef::try_acquire()`].
pub struct EmbeddedLocalSemaphorePermit {
    inner: NonNull<Inner>,
    permits: usize,
}

// The NonNull pointer is only dereferenced on the owning thread.
// All state access is confined to a single thread (!Send). No
// inconsistent state can be observed during unwind.
impl UnwindSafe for EmbeddedLocalSemaphorePermit {}
impl RefUnwindSafe for EmbeddedLocalSemaphorePermit {}

impl Drop for EmbeddedLocalSemaphorePermit {
    // Mutating drop to a no-op causes permits to leak.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        // SAFETY: The embedded() contract guarantees the container
        // outlives this permit.
        let inner = unsafe { self.inner.as_ref() };
        inner.release_permits(self.permits);
    }
}

/// Future returned by [`EmbeddedLocalSemaphoreRef::acquire()`] and
/// [`EmbeddedLocalSemaphoreRef::acquire_many()`].
pub struct EmbeddedLocalSemaphoreAcquireFuture {
    inner: NonNull<Inner>,
    permits: usize,

    awaiter: Awaiter,
}

// The NonNull pointer is only dereferenced on the owning thread.
// All state access is confined to a single thread (!Send). No
// inconsistent state can be observed during unwind.
impl UnwindSafe for EmbeddedLocalSemaphoreAcquireFuture {}
impl RefUnwindSafe for EmbeddedLocalSemaphoreAcquireFuture {}

impl Future for EmbeddedLocalSemaphoreAcquireFuture {
    type Output = EmbeddedLocalSemaphorePermit;

    fn poll(
        self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> Poll<EmbeddedLocalSemaphorePermit> {
        let waker = cx.waker().clone();

        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };

        // SAFETY: The container outlives this future per the
        // embedded() contract.
        let inner = unsafe { this.inner.as_ref() };
        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut this.awaiter) };
        // SAFETY: poll_acquire requires single-threaded access,
        // which LocalSemaphore guarantees (!Send).
        match unsafe { inner.poll_acquire(awaiter, this.permits, waker) } {
            Poll::Ready(()) => Poll::Ready(EmbeddedLocalSemaphorePermit {
                inner: this.inner,
                permits: this.permits,
            }),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for EmbeddedLocalSemaphoreAcquireFuture {
    // Inverting the is_registered() guard causes the Drop to hang
    // because it runs cleanup on an unregistered awaiter.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        // SAFETY: The container outlives this future per the
        // embedded() contract.
        let inner = unsafe { self.inner.as_ref() };
        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut self.awaiter) };
        // SAFETY: drop_acquire_wait requires single-threaded access,
        // which LocalSemaphore guarantees (!Send).
        unsafe {
            inner.drop_acquire_wait(awaiter, self.permits);
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
impl fmt::Debug for EmbeddedLocalSemaphoreRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedLocalSemaphoreRef")
            .finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for EmbeddedLocalSemaphorePermit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedLocalSemaphorePermit")
            .field("permits", &self.permits)
            .finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for EmbeddedLocalSemaphoreAcquireFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedLocalSemaphoreAcquireFuture")
            .field("permits", &self.permits)
            // SAFETY: Debug output is best-effort; no concurrent
            // mutation during formatting.
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::num::NonZero;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(LocalSemaphore: Clone, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(LocalSemaphore: Send, Sync);
    assert_impl_all!(LocalSemaphorePermit<'static>: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(LocalSemaphorePermit<'static>: Send, Sync, Clone);
    assert_impl_all!(LocalSemaphoreAcquireFuture<'static>: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(LocalSemaphoreAcquireFuture<'static>: Send, Sync, Unpin);

    assert_impl_all!(EmbeddedLocalSemaphore: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(EmbeddedLocalSemaphore: Send, Sync, Unpin);
    assert_impl_all!(EmbeddedLocalSemaphoreRef: Clone, Copy, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(EmbeddedLocalSemaphoreRef: Send, Sync);
    assert_impl_all!(EmbeddedLocalSemaphorePermit: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(EmbeddedLocalSemaphorePermit: Send, Sync, Clone);
    assert_impl_all!(EmbeddedLocalSemaphoreAcquireFuture: UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(EmbeddedLocalSemaphoreAcquireFuture: Send, Sync, Unpin);

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
        let permit = sem.try_acquire_many(NonZero::new(3).unwrap()).unwrap();
        assert!(sem.try_acquire_many(NonZero::new(3).unwrap()).is_none());
        assert!(sem.try_acquire_many(NonZero::new(2).unwrap()).is_some());
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
    fn all_waiters_eventually_acquire() {
        let sem = LocalSemaphore::boxed(1);
        let permit = sem.try_acquire().unwrap();

        let mut f1 = Box::pin(sem.acquire());
        let mut f2 = Box::pin(sem.acquire());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());

        drop(permit);

        // Poll both until both have acquired (order is unspecified).
        let mut acquired = 0_u32;
        while acquired < 2 {
            if let Poll::Ready(p) = f1.as_mut().poll(&mut cx) {
                acquired = acquired.checked_add(1).unwrap();
                drop(p);
            }
            if let Poll::Ready(p) = f2.as_mut().poll(&mut cx) {
                acquired = acquired.checked_add(1).unwrap();
                drop(p);
            }
        }
    }

    #[test]
    fn head_of_line_blocking() {
        let sem = LocalSemaphore::boxed(2);
        let _p1 = sem.try_acquire().unwrap();
        let _p2 = sem.try_acquire().unwrap();

        let mut f_big = Box::pin(sem.acquire_many(NonZero::new(2).unwrap()));
        let mut f_small = Box::pin(sem.acquire());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f_big.as_mut().poll(&mut cx).is_pending());
        assert!(f_small.as_mut().poll(&mut cx).is_pending());

        // Release both permits.
        drop(_p1);
        drop(_p2);

        // Poll both until both have acquired (order is unspecified).
        let mut acquired = 0_u32;
        while acquired < 2 {
            if let Poll::Ready(p) = f_big.as_mut().poll(&mut cx) {
                acquired = acquired.checked_add(1).unwrap();
                drop(p);
            }
            if let Poll::Ready(p) = f_small.as_mut().poll(&mut cx) {
                acquired = acquired.checked_add(1).unwrap();
                drop(p);
            }
        }
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
        let permit = sem.try_acquire_many(NonZero::new(5).unwrap()).unwrap();
        assert!(sem.try_acquire().is_none());
        drop(permit);
        assert!(sem.try_acquire().is_some());
    }

    #[test]
    fn try_acquire_many_exceeds_max() {
        let sem = LocalSemaphore::boxed(3);
        assert!(sem.try_acquire_many(NonZero::new(4).unwrap()).is_none());
        // Semaphore is untouched — still has 3 available.
        assert!(sem.try_acquire_many(NonZero::new(3).unwrap()).is_some());
    }

    #[test]
    fn acquire_many_all_at_once() {
        futures::executor::block_on(async {
            let sem = LocalSemaphore::boxed(5);
            let permit = sem.acquire_many(NonZero::new(5).unwrap()).await;
            assert!(sem.try_acquire().is_none());
            drop(permit);
            assert!(sem.try_acquire_many(NonZero::new(5).unwrap()).is_some());
        });
    }

    #[test]
    fn try_acquire_bypasses_waiter_queue() {
        let sem = LocalSemaphore::boxed(3);
        let _p1 = sem.try_acquire().unwrap();
        let _p2 = sem.try_acquire().unwrap();
        // 1 permit available, 2 held.

        // Register a waiter wanting 2 permits (more than available).
        let mut f1 = Box::pin(sem.acquire_many(NonZero::new(2).unwrap()));
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
        let big_permit = sem.try_acquire_many(NonZero::new(2).unwrap()).unwrap();

        let mut f1 = Box::pin(sem.acquire());
        let mut f2 = Box::pin(sem.acquire());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());

        // Release 2 permits at once — try_wake_one handles the
        // first waiter, wake_waiters must find the second.
        drop(big_permit);

        assert!(f1.as_mut().poll(&mut cx).is_ready());
        assert!(f2.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn reentrant_waker_does_not_alias() {
        use testing::ReentrantWakerData;

        let sem = LocalSemaphore::boxed(1);
        let sem_for_waker = sem.clone();

        let waker_data = ReentrantWakerData::new(move || {
            // Re-entrantly try to acquire from the same semaphore.
            drop(sem_for_waker.try_acquire());
        });
        // SAFETY: Data outlives waker, single-threaded test.
        let waker = unsafe { waker_data.waker() };
        let mut cx = task::Context::from_waker(&waker);

        let permit = sem.try_acquire().unwrap();
        let mut future = Box::pin(sem.acquire());
        assert!(future.as_mut().poll(&mut cx).is_pending());

        // Drop the permit — wakes the future via the re-entrant waker
        // which calls try_acquire().
        drop(permit);

        assert!(waker_data.was_woken());
    }
}
