use std::fmt;
use std::future::Future;
use std::hint;
use std::marker::PhantomPinned;
use std::num::NonZero;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{self, Poll, Waker};

use awaiter_set::{Awaiter, AwaiterSet};

use crate::constants::{NEVER_POISONED, ONE_PERMIT};

/// Thread-safe async semaphore.
///
/// Controls access to a shared resource by maintaining a pool of
/// permits. The [`acquire()`][Self::acquire] method returns a future
/// that resolves to a [`SemaphorePermit`] when a permit is available.
/// The permit is returned to the pool when dropped.
///
/// # Storage
///
/// The semaphore is a lightweight cloneable handle. All clones derived
/// from the same [`boxed()`][Self::boxed] call share the same
/// underlying state.
///
/// To avoid the heap allocation, use [`EmbeddedSemaphore`] with
/// [`embedded()`][Self::embedded] instead.
///
/// # Fairness
///
/// The order in which waiters are served is unspecified. There is
/// no head-of-line blocking: a waiter requesting many permits does
/// not prevent smaller requests from being satisfied.
///
/// # Examples
///
/// ```
/// use asynchroniz::Semaphore;
///
/// #[tokio::main]
/// async fn main() {
///     let sem = Semaphore::boxed(2);
///
///     tokio::spawn({
///         let sem = sem.clone();
///         async move {
///             let _permit = sem.acquire().await;
///         }
///     });
///
///     let _permit = sem.acquire().await;
///     // At most 2 futures can hold permits simultaneously.
/// }
/// ```
pub struct Semaphore {
    inner: Arc<SemaphoreInner>,
}

impl Clone for Semaphore {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

struct SemaphoreInner {
    // Atomic permit count for fast-path try_acquire/release.
    available: AtomicUsize,
    // Whether any futures are registered in the waiter set. When
    // false, release() can skip the mutex entirely.
    has_waiters: AtomicBool,
    slow: Mutex<SlowState>,
}

struct SlowState {
    waiters: AwaiterSet,
}

// Mutating release_permits to a no-op causes acquire futures to hang.
#[cfg_attr(test, mutants::skip)]
fn release_permits(inner: &SemaphoreInner, count: usize) {
    // Add permits atomically.
    inner.available.fetch_add(count, Ordering::Release);

    // Fast path: no waiters → nothing to wake.
    if !inner.has_waiters.load(Ordering::Acquire) {
        return;
    }

    // Slow path: wake satisfiable waiters under the mutex.
    let mut slow = inner.slow.lock().expect(NEVER_POISONED);
    loop {
        let available = &inner.available;
        let predicate = |awaiter: &Awaiter| {
            // SAFETY: We hold the mutex.
            let requested = unsafe { awaiter.user_data() };
            let current = available.load(Ordering::Relaxed);
            if requested <= current {
                // Deduct permits atomically. We are under the mutex
                // so no other thread is modifying the waiter set,
                // but another thread may do a concurrent try_acquire
                // on the atomic. Use a CAS loop.
                let mut cur = current;
                loop {
                    match available.compare_exchange_weak(
                        cur,
                        cur.wrapping_sub(requested),
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => return true,
                        Err(actual) => {
                            if actual < requested {
                                return false;
                            }
                            cur = actual;
                        }
                    }
                }
            } else {
                false
            }
        };
        // SAFETY: We hold the mutex.
        let waker = unsafe { slow.waiters.notify_one_if(predicate) };

        if let Some(w) = waker {
            // Update has_waiters if the set is now empty.
            // SAFETY: We hold the mutex.
            if unsafe { slow.waiters.is_empty() } {
                inner.has_waiters.store(false, Ordering::Release);
            }
            // Wake outside the critical loop but inside the mutex
            // (no mainstream runtime re-enters on wake).
            w.wake();
        } else {
            break;
        }
    }
}

// Mutating try_acquire_inner to always return false breaks tests.
#[cfg_attr(test, mutants::skip)]
fn try_acquire_inner(inner: &SemaphoreInner, permits: usize) -> bool {
    // Atomic CAS loop — no mutex needed.
    let mut current = inner.available.load(Ordering::Acquire);
    loop {
        if current < permits {
            return false;
        }
        match inner.available.compare_exchange_weak(
            current,
            current.wrapping_sub(permits),
            Ordering::AcqRel,
            Ordering::Relaxed,
        ) {
            Ok(_) => return true,
            Err(actual) => current = actual,
        }
    }
}

/// Shared poll logic for both `SemaphoreAcquireFuture` and
/// `EmbeddedSemaphoreAcquireFuture`.
///
/// # Safety
///
/// * The awaiter must belong to a future created from this semaphore.
unsafe fn poll_acquire(
    inner: &SemaphoreInner,
    mut awaiter: Pin<&mut Awaiter>,
    permits: usize,
    waker: Waker,
) -> Poll<()> {
    // Fast path: try atomic CAS before touching the mutex.
    if try_acquire_inner(inner, permits) {
        return Poll::Ready(());
    }

    // Check notification atomically — no mutex needed.
    if awaiter.as_ref().take_notification() {
        return Poll::Ready(());
    }

    // Give the permit releaser a chance before acquiring the mutex.
    hint::spin_loop();
    if try_acquire_inner(inner, permits) {
        return Poll::Ready(());
    }

    let mut slow = inner.slow.lock().expect(NEVER_POISONED);

    // Re-check notification under the mutex.
    if awaiter.as_ref().take_notification() {
        return Poll::Ready(());
    }

    // Register or update the waker. Set has_waiters before the
    // final permit re-check to close the race with a concurrent
    // release_permits that could observe has_waiters=false.
    inner.has_waiters.store(true, Ordering::Release);

    // Re-check permits after setting has_waiters. A concurrent
    // release that ran between our earlier check and the store
    // would have added permits without entering the slow path.
    // We catch that here if no other waiter is queued.
    if !awaiter.is_registered()
        // SAFETY: We hold the mutex.
        && unsafe { slow.waiters.is_empty() }
        && try_acquire_inner(inner, permits)
    {
        return Poll::Ready(());
    }

    // SAFETY: We hold the mutex, awaiter is pinned.
    unsafe {
        slow.waiters
            .register_with_data(awaiter.as_mut(), waker, permits);
    }
    Poll::Pending
}

/// Shared drop logic for both acquire future types.
///
/// # Safety
///
/// Same requirements as [`poll_acquire`].
unsafe fn drop_acquire_wait(
    inner: &SemaphoreInner,
    mut awaiter: Pin<&mut Awaiter>,
    permits: usize,
) {
    if !awaiter.is_registered() {
        return;
    }

    let mut slow = inner.slow.lock().expect(NEVER_POISONED);

    if awaiter.as_ref().is_notified() {
        // We were given permits but the future was cancelled.
        // Return the permits and try to wake other waiters.
        inner.available.fetch_add(permits, Ordering::Release);

        // Try to wake waiters with the returned permits.
        let available = &inner.available;
        let predicate = |aw: &Awaiter| {
            // SAFETY: We hold the mutex protecting the awaiter set.
            let requested = unsafe { aw.user_data() };
            let cur = available.load(Ordering::Relaxed);
            if requested <= cur {
                let mut c = cur;
                loop {
                    match available.compare_exchange_weak(
                        c,
                        c.wrapping_sub(requested),
                        Ordering::AcqRel,
                        Ordering::Relaxed,
                    ) {
                        Ok(_) => return true,
                        Err(actual) => {
                            if actual < requested {
                                return false;
                            }
                            c = actual;
                        }
                    }
                }
            } else {
                false
            }
        };
        // SAFETY: We hold the mutex.
        let waker = unsafe { slow.waiters.notify_one_if(predicate) };
        // SAFETY: We hold the mutex.
        if unsafe { slow.waiters.is_empty() } {
            inner.has_waiters.store(false, Ordering::Release);
        }
        drop(slow);
        if let Some(w) = waker {
            w.wake();
        }
    } else {
        // Not notified — just remove from the awaiter set.
        // SAFETY: We hold the mutex and the node is in the set.
        unsafe {
            slow.waiters.unregister(awaiter.as_mut());
        }
        // SAFETY: We hold the mutex.
        if unsafe { slow.waiters.is_empty() } {
            inner.has_waiters.store(false, Ordering::Release);
        }
    }
}

impl Semaphore {
    /// Creates a new semaphore with the given number of permits.
    ///
    /// The state is heap-allocated. Clone the handle to share the
    /// same semaphore. For caller-provided storage, see
    /// [`embedded()`][Self::embedded].
    ///
    /// # Examples
    ///
    /// ```
    /// use asynchroniz::Semaphore;
    ///
    /// let sem = Semaphore::boxed(3);
    /// assert!(sem.try_acquire().is_some());
    /// ```
    #[must_use]
    pub fn boxed(permits: usize) -> Self {
        Self {
            inner: Arc::new(SemaphoreInner {
                available: AtomicUsize::new(permits),
                has_waiters: AtomicBool::new(false),
                slow: Mutex::new(SlowState {
                    waiters: AwaiterSet::new(),
                }),
            }),
        }
    }

    /// Creates an instance that references the state in the
    /// [`EmbeddedSemaphore`].
    ///
    /// Calling this multiple times on the same container returns
    /// handles that all operate on the same shared state.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the [`EmbeddedSemaphore`] outlives
    /// all returned handles, all [`EmbeddedSemaphoreAcquireFuture`]s, and
    /// all [`EmbeddedSemaphorePermit`]s created from them.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::pin::pin;
    ///
    /// use asynchroniz::{EmbeddedSemaphore, Semaphore};
    ///
    /// # futures::executor::block_on(async {
    /// let container = pin!(EmbeddedSemaphore::new(1));
    ///
    /// // SAFETY: The container outlives the handle and all permits.
    /// let sem = unsafe { Semaphore::embedded(container.as_ref()) };
    ///
    /// let permit = sem.acquire().await;
    /// assert!(sem.try_acquire().is_none());
    /// drop(permit);
    /// assert!(sem.try_acquire().is_some());
    /// # });
    /// ```
    #[must_use]
    pub unsafe fn embedded(place: Pin<&EmbeddedSemaphore>) -> EmbeddedSemaphoreRef {
        let inner = NonNull::from(&place.get_ref().inner);
        EmbeddedSemaphoreRef { inner }
    }

    /// Returns a future that resolves to a [`SemaphorePermit`] when
    /// a single permit is available.
    ///
    /// # Examples
    ///
    /// ```
    /// use asynchroniz::Semaphore;
    ///
    /// # futures::executor::block_on(async {
    /// let sem = Semaphore::boxed(1);
    /// let permit = sem.acquire().await;
    /// assert!(sem.try_acquire().is_none());
    /// drop(permit);
    /// assert!(sem.try_acquire().is_some());
    /// # });
    /// ```
    #[must_use]
    pub fn acquire(&self) -> SemaphoreAcquireFuture<'_> {
        self.acquire_many(ONE_PERMIT)
    }

    /// Returns a future that resolves to a [`SemaphorePermit`]
    /// holding `permits` permits.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::num::NonZero;
    ///
    /// use asynchroniz::Semaphore;
    ///
    /// # futures::executor::block_on(async {
    /// let sem = Semaphore::boxed(5);
    /// let permit = sem.acquire_many(NonZero::new(3).unwrap()).await;
    ///
    /// // Only 2 permits remain.
    /// assert!(sem.try_acquire_many(NonZero::new(3).unwrap()).is_none());
    /// assert!(sem.try_acquire_many(NonZero::new(2).unwrap()).is_some());
    /// # });
    /// ```
    #[must_use]
    pub fn acquire_many(&self, permits: NonZero<usize>) -> SemaphoreAcquireFuture<'_> {
        SemaphoreAcquireFuture {
            inner: &self.inner,
            permits: permits.get(),
            awaiter: Awaiter::new(),
        }
    }

    /// Attempts to acquire a single permit without blocking.
    ///
    /// Returns [`Some(SemaphorePermit)`][SemaphorePermit] if a permit
    /// was available, or [`None`] otherwise.
    ///
    /// # Examples
    ///
    /// ```
    /// use asynchroniz::Semaphore;
    ///
    /// let sem = Semaphore::boxed(1);
    /// let permit = sem.try_acquire().unwrap();
    ///
    /// // No permits left.
    /// assert!(sem.try_acquire().is_none());
    /// ```
    #[must_use]
    // Mutating try_acquire to always return None breaks tests.
    #[cfg_attr(test, mutants::skip)]
    pub fn try_acquire(&self) -> Option<SemaphorePermit<'_>> {
        self.try_acquire_many(ONE_PERMIT)
    }

    /// Attempts to acquire `permits` permits without blocking.
    ///
    /// Returns [`Some(SemaphorePermit)`][SemaphorePermit] if enough
    /// permits were available, or [`None`] otherwise.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::num::NonZero;
    ///
    /// use asynchroniz::Semaphore;
    ///
    /// let sem = Semaphore::boxed(5);
    /// let permit = sem.try_acquire_many(NonZero::new(3).unwrap()).unwrap();
    ///
    /// // Only 2 remain.
    /// assert!(sem.try_acquire_many(NonZero::new(3).unwrap()).is_none());
    /// assert!(sem.try_acquire_many(NonZero::new(2).unwrap()).is_some());
    /// ```
    #[must_use]
    // Mutating try_acquire_many to always return None breaks tests.
    #[cfg_attr(test, mutants::skip)]
    pub fn try_acquire_many(&self, permits: NonZero<usize>) -> Option<SemaphorePermit<'_>> {
        let permits = permits.get();

        if try_acquire_inner(&self.inner, permits) {
            Some(SemaphorePermit {
                inner: &self.inner,
                permits,
            })
        } else {
            None
        }
    }
}

/// RAII permit returned by [`Semaphore::acquire()`] and
/// [`Semaphore::try_acquire()`].
///
/// The permit is returned to the semaphore when dropped.
pub struct SemaphorePermit<'a> {
    inner: &'a SemaphoreInner,
    permits: usize,
}

impl Drop for SemaphorePermit<'_> {
    // Mutating drop to a no-op causes the permits to leak.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        release_permits(self.inner, self.permits);
    }
}

// All state access is serialized by the internal Mutex. No
// inconsistent state can be observed during unwind.
impl UnwindSafe for SemaphorePermit<'_> {}
impl RefUnwindSafe for SemaphorePermit<'_> {}

/// Future returned by [`Semaphore::acquire()`] and
/// [`Semaphore::acquire_many()`].
///
/// Completes with a [`SemaphorePermit`] when enough permits are
/// available.
pub struct SemaphoreAcquireFuture<'a> {
    inner: &'a SemaphoreInner,
    permits: usize,

    awaiter: Awaiter,
}

// Marker trait impl.
// SAFETY: Awaiter is Send. All access to the awaiter is serialized
// by the semaphore's internal Mutex. The reference points to data
// behind an Arc that is Send + Sync.
unsafe impl Send for SemaphoreAcquireFuture<'_> {}

// All state access is serialized by the internal Mutex. No
// inconsistent state can be observed during unwind.
impl UnwindSafe for SemaphoreAcquireFuture<'_> {}
impl RefUnwindSafe for SemaphoreAcquireFuture<'_> {}

impl<'a> Future for SemaphoreAcquireFuture<'a> {
    type Output = SemaphorePermit<'a>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<SemaphorePermit<'a>> {
        // Clone the waker before acquiring the lock so a panicking
        // clone cannot poison the mutex.
        let waker = cx.waker().clone();

        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };

        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut this.awaiter) };
        // SAFETY: The state field is the semaphore state this awaiter registers
        // with.
        match unsafe { poll_acquire(this.inner, awaiter, this.permits, waker) } {
            Poll::Ready(()) => Poll::Ready(SemaphorePermit {
                inner: this.inner,
                permits: this.permits,
            }),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for SemaphoreAcquireFuture<'_> {
    // Inverting the is_registered() guard causes the Drop to hang
    // because it runs cleanup on an unregistered awaiter.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut self.awaiter) };
        // SAFETY: The state field is the mutex this awaiter was
        // registered with.
        unsafe { drop_acquire_wait(self.inner, awaiter, self.permits) }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for Semaphore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Semaphore").finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for SemaphorePermit<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SemaphorePermit")
            .field("permits", &self.permits)
            .finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for SemaphoreAcquireFuture<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SemaphoreAcquireFuture")
            .field("permits", &self.permits)
            // SAFETY: Debug output is best-effort; no concurrent
            // mutation during formatting.
            .finish_non_exhaustive()
    }
}

/// Inline storage for semaphore state, avoiding heap allocation.
///
/// Pin the container, then call [`Semaphore::embedded()`] to obtain a
/// [`EmbeddedSemaphoreRef`] reference that operates on the embedded state.
///
/// # Examples
///
/// ```
/// use std::pin::pin;
///
/// use asynchroniz::{EmbeddedSemaphore, Semaphore};
///
/// # futures::executor::block_on(async {
/// let container = pin!(EmbeddedSemaphore::new(2));
///
/// // SAFETY: The container outlives the handle and all permits.
/// let sem = unsafe { Semaphore::embedded(container.as_ref()) };
///
/// let p1 = sem.acquire().await;
/// let p2 = sem.acquire().await;
/// assert!(sem.try_acquire().is_none());
/// # });
/// ```
pub struct EmbeddedSemaphore {
    inner: SemaphoreInner,
    _pinned: PhantomPinned,
}

impl EmbeddedSemaphore {
    /// Creates a new embedded semaphore container with the given
    /// number of permits.
    #[must_use]
    pub fn new(permits: usize) -> Self {
        Self {
            inner: SemaphoreInner {
                available: AtomicUsize::new(permits),
                has_waiters: AtomicBool::new(false),
                slow: Mutex::new(SlowState {
                    waiters: AwaiterSet::new(),
                }),
            },
            _pinned: PhantomPinned,
        }
    }
}

/// Reference to an [`EmbeddedSemaphore`].
///
/// Created via [`Semaphore::embedded()`]. The caller is responsible
/// for ensuring the [`EmbeddedSemaphore`] outlives all handles,
/// acquire futures, and permits.
///
/// The API is identical to [`Semaphore`].
#[derive(Clone, Copy)]
pub struct EmbeddedSemaphoreRef {
    inner: NonNull<SemaphoreInner>,
}

// Marker trait impl.
// SAFETY: The NonNull<SemaphoreInner> only points to a value whose
// access is serialized by the internal Mutex.
unsafe impl Send for EmbeddedSemaphoreRef {}

// Marker trait impl.
// SAFETY: Same as Send — all mutable access is mediated by the
// internal lock.
unsafe impl Sync for EmbeddedSemaphoreRef {}

// The NonNull pointer is only dereferenced while the internal Mutex is
// held. No inconsistent state can be observed during unwind.
impl UnwindSafe for EmbeddedSemaphoreRef {}
impl RefUnwindSafe for EmbeddedSemaphoreRef {}

impl EmbeddedSemaphoreRef {
    fn inner(&self) -> &SemaphoreInner {
        // SAFETY: The caller of `embedded()` guarantees the
        // container outlives this handle.
        unsafe { self.inner.as_ref() }
    }

    /// Returns a future that resolves to a [`EmbeddedSemaphorePermit`]
    /// when a single permit is available.
    #[must_use]
    pub fn acquire(&self) -> EmbeddedSemaphoreAcquireFuture {
        self.acquire_many(ONE_PERMIT)
    }

    /// Returns a future that resolves to a [`EmbeddedSemaphorePermit`]
    /// holding `permits` permits.
    #[must_use]
    pub fn acquire_many(&self, permits: NonZero<usize>) -> EmbeddedSemaphoreAcquireFuture {
        EmbeddedSemaphoreAcquireFuture {
            inner: self.inner,
            permits: permits.get(),
            awaiter: Awaiter::new(),
        }
    }

    /// Attempts to acquire a single permit without blocking.
    #[must_use]
    // Mutating try_acquire to always return None breaks tests.
    #[cfg_attr(test, mutants::skip)]
    pub fn try_acquire(&self) -> Option<EmbeddedSemaphorePermit> {
        self.try_acquire_many(ONE_PERMIT)
    }

    /// Attempts to acquire `permits` permits without blocking.
    #[must_use]
    // Mutating try_acquire_many to always return None breaks tests.
    #[cfg_attr(test, mutants::skip)]
    pub fn try_acquire_many(&self, permits: NonZero<usize>) -> Option<EmbeddedSemaphorePermit> {
        let permits = permits.get();

        if try_acquire_inner(self.inner(), permits) {
            Some(EmbeddedSemaphorePermit {
                inner: self.inner,
                permits,
            })
        } else {
            None
        }
    }
}

/// RAII permit returned by [`EmbeddedSemaphoreRef::acquire()`] and
/// [`EmbeddedSemaphoreRef::try_acquire()`].
///
/// The permit is returned to the semaphore when dropped.
pub struct EmbeddedSemaphorePermit {
    inner: NonNull<SemaphoreInner>,
    permits: usize,
}

// Marker trait impl.
// SAFETY: The permit only holds a NonNull to a Mutex-protected value
// and a plain usize. Sending across threads is safe.
unsafe impl Send for EmbeddedSemaphorePermit {}

// Marker trait impl.
// SAFETY: Sharing &EmbeddedSemaphorePermit gives no mutable access.
unsafe impl Sync for EmbeddedSemaphorePermit {}

// The NonNull pointer is only dereferenced while the internal Mutex is
// held. No inconsistent state can be observed during unwind.
impl UnwindSafe for EmbeddedSemaphorePermit {}
impl RefUnwindSafe for EmbeddedSemaphorePermit {}

impl Drop for EmbeddedSemaphorePermit {
    // Mutating drop to a no-op causes permits to leak.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        // SAFETY: The embedded() contract guarantees the container
        // outlives this permit.
        let inner = unsafe { self.inner.as_ref() };
        release_permits(inner, self.permits);
    }
}

/// Future returned by [`EmbeddedSemaphoreRef::acquire()`] and
/// [`EmbeddedSemaphoreRef::acquire_many()`].
///
/// Completes with a [`EmbeddedSemaphorePermit`] when enough permits are
/// available.
pub struct EmbeddedSemaphoreAcquireFuture {
    inner: NonNull<SemaphoreInner>,
    permits: usize,

    awaiter: Awaiter,
}

// Marker trait impl.
// SAFETY: Same reasoning as SemaphoreAcquireFuture — all awaiter access
// is protected by the internal Mutex.
unsafe impl Send for EmbeddedSemaphoreAcquireFuture {}

// The NonNull pointer is only dereferenced while the internal Mutex is
// held. No inconsistent state can be observed during unwind.
impl UnwindSafe for EmbeddedSemaphoreAcquireFuture {}
impl RefUnwindSafe for EmbeddedSemaphoreAcquireFuture {}

impl Future for EmbeddedSemaphoreAcquireFuture {
    type Output = EmbeddedSemaphorePermit;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<EmbeddedSemaphorePermit> {
        // Clone the waker before acquiring the lock so a panicking
        // clone cannot poison the mutex.
        let waker = cx.waker().clone();

        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };

        // SAFETY: The container outlives this future per the
        // embedded() contract.
        let inner = unsafe { this.inner.as_ref() };
        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut this.awaiter) };
        // SAFETY: The state is the mutex this awaiter registers with.
        match unsafe { poll_acquire(inner, awaiter, this.permits, waker) } {
            Poll::Ready(()) => Poll::Ready(EmbeddedSemaphorePermit {
                inner: this.inner,
                permits: this.permits,
            }),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for EmbeddedSemaphoreAcquireFuture {
    // Inverting the is_registered() guard causes the Drop to hang
    // because it runs cleanup on an unregistered awaiter.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        // SAFETY: The container outlives this future per the
        // embedded() contract.
        let inner = unsafe { self.inner.as_ref() };
        // SAFETY: The awaiter is pinned inside this future and not moved.
        let awaiter = unsafe { Pin::new_unchecked(&mut self.awaiter) };
        // SAFETY: The state is the mutex this awaiter was registered
        // with.
        unsafe { drop_acquire_wait(inner, awaiter, self.permits) }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for EmbeddedSemaphore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedSemaphore").finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for EmbeddedSemaphoreRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedSemaphoreRef")
            .finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for EmbeddedSemaphorePermit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedSemaphorePermit")
            .field("permits", &self.permits)
            .finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for EmbeddedSemaphoreAcquireFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedSemaphoreAcquireFuture")
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
    use std::sync::Barrier;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::{iter, thread};

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(Semaphore: Send, Sync, Clone, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(SemaphorePermit<'static>: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(SemaphorePermit<'static>: Clone);
    assert_impl_all!(SemaphoreAcquireFuture<'static>: Send, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(SemaphoreAcquireFuture<'static>: Sync, Unpin);

    assert_impl_all!(EmbeddedSemaphore: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(EmbeddedSemaphore: Unpin);
    assert_impl_all!(EmbeddedSemaphoreRef: Send, Sync, Clone, Copy, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(EmbeddedSemaphorePermit: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(EmbeddedSemaphorePermit: Clone);
    assert_impl_all!(EmbeddedSemaphoreAcquireFuture: Send, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(EmbeddedSemaphoreAcquireFuture: Sync, Unpin);

    #[test]
    fn acquire_and_release() {
        let sem = Semaphore::boxed(1);
        let permit = sem.try_acquire().unwrap();
        assert!(sem.try_acquire().is_none());
        drop(permit);
        assert!(sem.try_acquire().is_some());
    }

    #[test]
    fn multiple_permits() {
        let sem = Semaphore::boxed(3);
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
        let sem = Semaphore::boxed(5);
        let permit = sem.try_acquire_many(NonZero::new(3).unwrap()).unwrap();
        assert!(sem.try_acquire_many(NonZero::new(3).unwrap()).is_none());
        assert!(sem.try_acquire_many(NonZero::new(2).unwrap()).is_some());
        drop(permit);
    }

    #[test]
    fn clone_shares_state() {
        let a = Semaphore::boxed(1);
        let b = a.clone();
        let permit = a.try_acquire().unwrap();
        assert!(b.try_acquire().is_none());
        drop(permit);
        assert!(b.try_acquire().is_some());
    }

    #[test]
    fn acquire_completes_when_available() {
        futures::executor::block_on(async {
            let sem = Semaphore::boxed(1);
            let permit = sem.acquire().await;
            assert!(sem.try_acquire().is_none());
            drop(permit);
        });
    }

    #[test]
    fn acquire_completes_after_release() {
        let sem = Semaphore::boxed(1);
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
        let sem = Semaphore::boxed(1);
        let permit = sem.try_acquire().unwrap();

        let mut f1 = Box::pin(sem.acquire());
        let mut f2 = Box::pin(sem.acquire());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());

        drop(permit);

        // Poll both until both have acquired. Order is unspecified.
        let mut acquired = 0_u32;
        let mut futures = [f1, f2];
        while acquired < 2 {
            for f in &mut futures {
                if let Poll::Ready(p) = f.as_mut().poll(&mut cx) {
                    acquired = acquired.checked_add(1).unwrap();
                    drop(p);
                }
            }
        }
    }

    #[test]
    fn head_of_line_blocking() {
        let sem = Semaphore::boxed(2);
        let _p1 = sem.try_acquire().unwrap();
        let _p2 = sem.try_acquire().unwrap();

        // f_big wants 2 permits, f_small wants 1. The head waiter
        // (whichever it is) blocks the other until it can be
        // satisfied.
        let mut f_big = Box::pin(sem.acquire_many(NonZero::new(2).unwrap()));
        let mut f_small = Box::pin(sem.acquire());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f_big.as_mut().poll(&mut cx).is_pending());
        assert!(f_small.as_mut().poll(&mut cx).is_pending());

        // Release 1 permit. Neither can proceed because the head
        // waiter (either needing 2 or needing 1) is checked first.
        // If the head needs 2, it blocks. If the head needs 1, it
        // acquires and the other still needs more releases.
        drop(_p1);

        // Release the second permit.
        drop(_p2);

        // Poll both until both have acquired (order is unspecified).
        let mut acquired = 0_u32;
        let mut futures: [Pin<Box<dyn Future<Output = _>>>; 2] = [f_big, f_small];
        while acquired < 2 {
            for f in &mut futures {
                if let Poll::Ready(p) = f.as_mut().poll(&mut cx) {
                    acquired = acquired.checked_add(1).unwrap();
                    drop(p);
                }
            }
        }
    }

    #[test]
    fn release_wakes_multiple_waiters() {
        let sem = Semaphore::boxed(3);
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
    fn cancelled_waiter_returns_permits() {
        let sem = Semaphore::boxed(1);
        let permit = sem.try_acquire().unwrap();

        let mut f1 = Box::pin(sem.acquire());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());
        drop(permit);
        // f1 is notified but not polled.
        drop(f1);

        // Permit should be returned.
        assert!(sem.try_acquire().is_some());
    }

    #[test]
    fn notified_then_dropped_wakes_next() {
        let sem = Semaphore::boxed(1);
        let permit = sem.try_acquire().unwrap();

        let mut f1 = Box::pin(sem.acquire());
        let mut f2 = Box::pin(sem.acquire());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());

        drop(permit);
        // f1 is notified. Drop it without polling.
        drop(f1);

        // f2 should now be satisfiable.
        assert!(f2.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn drop_unpolled_future_is_safe() {
        let sem = Semaphore::boxed(1);
        {
            let _future = sem.acquire();
        }
        assert!(sem.try_acquire().is_some());
    }

    #[test]
    fn zero_initial_permits() {
        let sem = Semaphore::boxed(0);
        assert!(sem.try_acquire().is_none());
    }

    #[test]
    fn try_acquire_many_exact_max() {
        let sem = Semaphore::boxed(5);
        let permit = sem.try_acquire_many(NonZero::new(5).unwrap()).unwrap();
        assert!(sem.try_acquire().is_none());
        drop(permit);
        assert!(sem.try_acquire().is_some());
    }

    #[test]
    fn try_acquire_many_exceeds_max() {
        let sem = Semaphore::boxed(3);
        assert!(sem.try_acquire_many(NonZero::new(4).unwrap()).is_none());
        // Semaphore is untouched — still has 3 available.
        assert!(sem.try_acquire_many(NonZero::new(3).unwrap()).is_some());
    }

    #[test]
    fn acquire_many_all_at_once() {
        futures::executor::block_on(async {
            let sem = Semaphore::boxed(5);
            let permit = sem.acquire_many(NonZero::new(5).unwrap()).await;
            assert!(sem.try_acquire().is_none());
            drop(permit);
            assert!(sem.try_acquire_many(NonZero::new(5).unwrap()).is_some());
        });
    }

    #[test]
    fn try_acquire_bypasses_waiter_queue() {
        let sem = Semaphore::boxed(3);
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
    fn multiple_sequential_cancellations() {
        let sem = Semaphore::boxed(1);
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
        let sem = Semaphore::boxed(1);
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
    fn acquire_from_another_thread() {
        testing::with_watchdog(|| {
            let sem = Semaphore::boxed(1);
            let handle = sem;
            let barrier = Arc::new(Barrier::new(2));
            let b2 = Arc::clone(&barrier);

            let t = thread::spawn(move || {
                b2.wait();
                let _permit = futures::executor::block_on(handle.acquire());
            });

            barrier.wait();
            t.join().unwrap();
        });
    }

    #[test]
    fn concurrent_acquire_respects_limit() {
        testing::with_watchdog(|| {
            let sem = Semaphore::boxed(2);
            let thread_count = 4;
            let barrier = Arc::new(Barrier::new(thread_count + 1));
            let max_concurrent = Arc::new(AtomicUsize::new(0));
            let current = Arc::new(AtomicUsize::new(0));

            let handles: Vec<_> = iter::repeat_with(|| {
                let s = sem.clone();
                let b = Arc::clone(&barrier);
                let mc = Arc::clone(&max_concurrent);
                let cur = Arc::clone(&current);

                thread::spawn(move || {
                    b.wait();
                    for _ in 0..50 {
                        let _permit = futures::executor::block_on(s.acquire());
                        let c = cur.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
                        // Update max seen.
                        let mut prev = mc.load(Ordering::Relaxed);
                        while c > prev {
                            match mc.compare_exchange_weak(
                                prev,
                                c,
                                Ordering::Relaxed,
                                Ordering::Relaxed,
                            ) {
                                Ok(_) => break,
                                Err(p) => prev = p,
                            }
                        }
                        cur.fetch_sub(1, Ordering::SeqCst);
                    }
                })
            })
            .take(thread_count)
            .collect();

            barrier.wait();
            for h in handles {
                h.join().unwrap();
            }

            assert!(max_concurrent.load(Ordering::Relaxed) <= 2);
        });
    }

    #[test]
    fn embedded_acquire_and_release() {
        futures::executor::block_on(async {
            let container = Box::pin(EmbeddedSemaphore::new(1));
            // SAFETY: The container outlives all handles.
            let sem = unsafe { Semaphore::embedded(container.as_ref()) };

            let permit = sem.acquire().await;
            assert!(sem.try_acquire().is_none());
            drop(permit);
            assert!(sem.try_acquire().is_some());
        });
    }

    #[test]
    fn embedded_clone_shares_state() {
        let container = Box::pin(EmbeddedSemaphore::new(1));
        // SAFETY: The container outlives all handles.
        let a = unsafe { Semaphore::embedded(container.as_ref()) };
        let b = a;

        let permit = a.try_acquire().unwrap();
        assert!(b.try_acquire().is_none());
        drop(permit);
        assert!(b.try_acquire().is_some());
    }

    #[test]
    fn embedded_drop_future_while_waiting() {
        futures::executor::block_on(async {
            let container = Box::pin(EmbeddedSemaphore::new(1));
            // SAFETY: The container outlives all handles.
            let sem = unsafe { Semaphore::embedded(container.as_ref()) };

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
        let container = Box::pin(EmbeddedSemaphore::new(1));
        // SAFETY: The container outlives all handles.
        let sem = unsafe { Semaphore::embedded(container.as_ref()) };

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
        let container = Box::pin(EmbeddedSemaphore::new(1));
        // SAFETY: The container outlives all handles.
        let sem = unsafe { Semaphore::embedded(container.as_ref()) };

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
    fn embedded_from_another_thread() {
        testing::with_watchdog(|| {
            let container = Box::pin(EmbeddedSemaphore::new(1));
            // SAFETY: The container outlives all handles.
            let sem = unsafe { Semaphore::embedded(container.as_ref()) };
            let handle = sem;
            let barrier = Arc::new(Barrier::new(2));
            let b2 = Arc::clone(&barrier);

            let t = thread::spawn(move || {
                b2.wait();
                let _permit = futures::executor::block_on(handle.acquire());
            });

            barrier.wait();
            t.join().unwrap();
        });
    }

    #[test]
    fn multi_permit_release_wakes_multiple_single_permit_waiters() {
        // Releasing a multi-permit hold should wake multiple
        // single-permit waiters via the wake_waiters loop.
        let sem = Semaphore::boxed(2);
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

        let sem = Semaphore::boxed(1);
        let sem_for_waker = sem.clone();

        let waker_data = ReentrantWakerData::new(move || {
            // Re-entrantly try to acquire from the same
            // semaphore.
            drop(sem_for_waker.try_acquire());
        });
        // SAFETY: Data outlives waker, single-threaded test.
        let waker = unsafe { waker_data.waker() };
        let mut cx = task::Context::from_waker(&waker);

        let permit = sem.try_acquire().unwrap();
        let mut future = Box::pin(sem.acquire());
        assert!(future.as_mut().poll(&mut cx).is_pending());

        // Drop the permit — wakes the future via the
        // re-entrant waker which calls try_acquire().
        drop(permit);

        assert!(waker_data.was_woken());
    }
}
