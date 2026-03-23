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

/// Thread-safe async semaphore.
///
/// Controls access to a shared resource by maintaining a pool of
/// permits. The [`acquire()`][Self::acquire] method returns a future
/// that resolves to a [`SemaphorePermit`] when a permit is available.
/// The permit is returned to the pool when dropped.
///
/// The semaphore is a lightweight cloneable handle. All clones derived
/// from the same [`boxed()`][Self::boxed] call share the same
/// underlying state.
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
/// use asynchroniz::Semaphore;
///
/// #[tokio::main]
/// async fn main() {
///     let sem = Semaphore::boxed(2);
///     let handle = sem.clone();
///
///     tokio::spawn(async move {
///         let _permit = handle.acquire().await;
///         // Hold one permit in the background task.
///     });
///
///     let _permit = sem.acquire().await;
///     // At most 2 tasks can hold permits simultaneously.
/// }
/// ```
pub struct Semaphore {
    inner: Arc<SemaphoreInner>,
}

impl Clone for Semaphore {
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

struct SemaphoreInner {
    state: Mutex<SemaphoreState>,
}

struct SemaphoreState {
    available: usize,
    waiters: WaiterList,
}

// SAFETY: `SemaphoreState` contains raw pointers (via `WaiterList`)
// which are `!Send` by default. Sending is safe because the pointers
// are only dereferenced while the `Mutex` is held.
// Marker trait impl.
unsafe impl Send for SemaphoreState {}

// Marker trait impl.
impl UnwindSafe for SemaphoreInner {}
// Marker trait impl.
impl RefUnwindSafe for SemaphoreInner {}

// Mutating release_permits to a no-op causes acquire futures to hang.
#[cfg_attr(test, mutants::skip)]
fn release_permits(state_mutex: &Mutex<SemaphoreState>, count: usize) {
    // Combine the permit addition and the first waiter check into a
    // single lock scope, avoiding a second lock acquisition when no
    // waiters are queued (the common uncontended case).
    let (waker, head_satisfiable) = {
        let mut state = state_mutex.lock().expect(NEVER_POISONED);
        state.available = state
            .available
            .checked_add(count)
            .expect("permit count overflow is unreachable");
        let head = state.waiters.head();
        // SAFETY: We hold the lock and head is non-null.
        let satisfiable = !head.is_null() && state.available >= unsafe { (*head).user_data() };
        (try_wake_head(&mut state), satisfiable)
    };
    // If the head waiter was satisfiable, try_wake_head must have
    // returned a waker.
    debug_assert!(
        !head_satisfiable || waker.is_some(),
        "head waiter was satisfiable but try_wake_head returned None"
    );

    if let Some(w) = waker {
        w.wake();
        // We satisfied one waiter. If the release added more permits
        // than that waiter needed, more waiters may be satisfiable.
        wake_waiters(state_mutex);
    }
}

/// Tries to satisfy the head waiter, returning its waker if
/// successful.
///
/// Must be called while holding the state mutex.
fn try_wake_head(state: &mut SemaphoreState) -> Option<Waker> {
    let head = state.waiters.head();

    if head.is_null() {
        return None;
    }

    // SAFETY: Caller holds the lock and head is non-null.
    let requested = unsafe { (*head).user_data() };

    if state.available >= requested {
        state.available = state
            .available
            .checked_sub(requested)
            .expect("available >= requested was just checked");

        // SAFETY: Caller holds the lock.
        let node =
            unsafe { state.waiters.pop_front() }.expect("head was non-null so pop cannot fail");
        // SAFETY: Caller holds the lock and just popped this node.
        unsafe {
            (*node).set_notified();
        }
        // SAFETY: Same node, caller holds the lock.
        unsafe { (*node).take_waker() }
    } else {
        // Head-of-line blocking: not enough permits for the head
        // waiter.
        None
    }
}

/// Wakes queued waiters whose permit requests can now be satisfied.
///
/// Uses head-of-line blocking: only the head waiter is considered. If
/// the head waiter cannot be satisfied, no subsequent waiters are
/// tried even if they request fewer permits.
// Mutating wake_waiters to a no-op causes acquire futures to hang.
#[cfg_attr(test, mutants::skip)]
fn wake_waiters(state_mutex: &Mutex<SemaphoreState>) {
    loop {
        let waker = {
            let mut state = state_mutex.lock().expect(NEVER_POISONED);
            try_wake_head(&mut state)
        };

        if let Some(w) = waker {
            w.wake();
        } else {
            break;
        }
    }
}

// Mutating try_acquire_inner to always return false breaks tests.
#[cfg_attr(test, mutants::skip)]
fn try_acquire_inner(state_mutex: &Mutex<SemaphoreState>, permits: usize) -> bool {
    let mut state = state_mutex.lock().expect(NEVER_POISONED);
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

/// Shared poll logic for both `SemaphoreAcquireFuture` and
/// `RawSemaphoreAcquireFuture`.
///
/// # Safety
///
/// * The `node` must be pinned and must remain at the same memory
///   address for the lifetime of the acquire future.
/// * The `state_mutex` must protect the waiter list that this node
///   is (or will be) registered with.
unsafe fn poll_acquire(
    state_mutex: &Mutex<SemaphoreState>,
    node: &UnsafeCell<WaiterNode>,
    registered: &mut bool,
    permits: usize,
    waker: &Waker,
) -> Poll<()> {
    let node_ptr = node.get();

    let mut state = state_mutex.lock().expect(NEVER_POISONED);

    // Check if we were directly notified by release_permits()
    // (it popped us from the list and reserved our permits).
    // SAFETY: We hold the lock.
    if unsafe { (*node_ptr).is_notified() } {
        *registered = false;
        return Poll::Ready(());
    }

    if !*registered && state.available >= permits {
        // Permits are available and we are not yet registered
        // (first poll fast path).
        state.available = state
            .available
            .checked_sub(permits)
            .expect("available >= permits was just checked");
        Poll::Ready(())
    } else {
        // Not enough permits or already registered — wait.
        // SAFETY: We hold the lock, node_ptr is from our pinned
        // UnsafeCell.
        unsafe {
            (*node_ptr).store_waker(waker);
        }
        // SAFETY: We hold the lock, node_ptr is from our pinned
        // UnsafeCell.
        unsafe {
            (*node_ptr).set_user_data(permits);
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

/// Shared drop logic for both acquire future types.
///
/// # Safety
///
/// Same requirements as [`poll_acquire`].
unsafe fn drop_acquire_wait(
    state_mutex: &Mutex<SemaphoreState>,
    node: &UnsafeCell<WaiterNode>,
    permits: usize,
) {
    let node_ptr = node.get();
    let mut state = state_mutex.lock().expect(NEVER_POISONED);

    // SAFETY: We hold the lock.
    if unsafe { (*node_ptr).is_notified() } {
        // We were given permits but the future was cancelled.
        // Return the permits and try to wake other waiters, all
        // within the same lock scope.
        state.available = state
            .available
            .checked_add(permits)
            .expect("permit count overflow is unreachable");
        let waker = try_wake_head(&mut state);
        drop(state);
        if let Some(w) = waker {
            w.wake();
            wake_waiters(state_mutex);
        }
    } else {
        // Not notified — just remove from the waiter list.
        // SAFETY: We hold the lock and the node is in the list.
        unsafe {
            state.waiters.remove(node_ptr);
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
                state: Mutex::new(SemaphoreState {
                    available: permits,
                    waiters: WaiterList::new(),
                }),
            }),
        }
    }

    /// Creates a handle from an [`EmbeddedSemaphore`] container,
    /// avoiding heap allocation.
    ///
    /// Calling this multiple times on the same container is safe and
    /// returns handles that all operate on the same shared state,
    /// just like copying or cloning a [`RawSemaphore`].
    ///
    /// # Safety
    ///
    /// The caller must ensure that the [`EmbeddedSemaphore`] outlives
    /// all returned handles, all [`RawSemaphoreAcquireFuture`]s, and
    /// all [`RawSemaphorePermit`]s created from them.
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
    /// let sem =
    ///     unsafe { Semaphore::embedded(container.as_ref()) };
    ///
    /// let permit = sem.acquire().await;
    /// assert!(sem.try_acquire().is_none());
    /// drop(permit);
    /// assert!(sem.try_acquire().is_some());
    /// # });
    /// ```
    #[must_use]
    pub unsafe fn embedded(place: Pin<&EmbeddedSemaphore>) -> RawSemaphore {
        let inner = NonNull::from(&place.get_ref().inner);
        RawSemaphore { inner }
    }

    /// Returns a future that resolves to a [`SemaphorePermit`] when
    /// a single permit is available.
    ///
    /// # Cancellation safety
    ///
    /// If a future that has been notified is dropped before it is
    /// polled to completion, the permit is returned and any newly
    /// satisfiable waiters are woken.
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
        self.acquire_many(1)
    }

    /// Returns a future that resolves to a [`SemaphorePermit`]
    /// holding `permits` permits.
    ///
    /// # Panics
    ///
    /// Panics if `permits` is zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use asynchroniz::Semaphore;
    ///
    /// # futures::executor::block_on(async {
    /// let sem = Semaphore::boxed(5);
    /// let permit = sem.acquire_many(3).await;
    ///
    /// // Only 2 permits remain.
    /// assert!(sem.try_acquire_many(3).is_none());
    /// assert!(sem.try_acquire_many(2).is_some());
    /// # });
    /// ```
    #[must_use]
    pub fn acquire_many(&self, permits: usize) -> SemaphoreAcquireFuture<'_> {
        assert!(permits > 0, "cannot acquire zero permits");

        SemaphoreAcquireFuture {
            state: &self.inner.state,
            permits,
            node: UnsafeCell::new(WaiterNode::new()),
            registered: false,
            _pinned: PhantomPinned,
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
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn try_acquire(&self) -> Option<SemaphorePermit<'_>> {
        self.try_acquire_many(1)
    }

    /// Attempts to acquire `permits` permits without blocking.
    ///
    /// Returns [`Some(SemaphorePermit)`][SemaphorePermit] if enough
    /// permits were available, or [`None`] otherwise.
    ///
    /// # Panics
    ///
    /// Panics if `permits` is zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use asynchroniz::Semaphore;
    ///
    /// let sem = Semaphore::boxed(5);
    /// let permit = sem.try_acquire_many(3).unwrap();
    ///
    /// // Only 2 remain.
    /// assert!(sem.try_acquire_many(3).is_none());
    /// assert!(sem.try_acquire_many(2).is_some());
    /// ```
    #[must_use]
    // Mutating try_acquire_many to always return None breaks tests.
    #[cfg_attr(test, mutants::skip)]
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn try_acquire_many(&self, permits: usize) -> Option<SemaphorePermit<'_>> {
        assert!(permits > 0, "cannot acquire zero permits");

        if try_acquire_inner(&self.inner.state, permits) {
            Some(SemaphorePermit {
                state: &self.inner.state,
                permits,
            })
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------
// SemaphorePermit
// ---------------------------------------------------------------------------

/// RAII permit returned by [`Semaphore::acquire()`] and
/// [`Semaphore::try_acquire()`].
///
/// The permit is returned to the semaphore when dropped.
pub struct SemaphorePermit<'a> {
    state: &'a Mutex<SemaphoreState>,
    permits: usize,
}

// Marker trait impl.
impl UnwindSafe for SemaphorePermit<'_> {}
// Marker trait impl.
impl RefUnwindSafe for SemaphorePermit<'_> {}

impl Drop for SemaphorePermit<'_> {
    // Mutating drop to a no-op causes the permits to leak.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        release_permits(self.state, self.permits);
    }
}

// ---------------------------------------------------------------------------
// SemaphoreAcquireFuture
// ---------------------------------------------------------------------------

/// Future returned by [`Semaphore::acquire()`] and
/// [`Semaphore::acquire_many()`].
///
/// Completes with a [`SemaphorePermit`] when enough permits are
/// available.
pub struct SemaphoreAcquireFuture<'a> {
    state: &'a Mutex<SemaphoreState>,
    permits: usize,

    // Behind UnsafeCell so that raw pointers from the waiter list
    // can coexist with the &mut Self we obtain in poll() via
    // get_unchecked_mut().
    node: UnsafeCell<WaiterNode>,

    // Whether this future's node is currently in the waiter list.
    registered: bool,

    _pinned: PhantomPinned,
}

// Marker trait impl.
// SAFETY: All UnsafeCell<WaiterNode> fields are accessed exclusively
// under the semaphore's internal Mutex. The reference points to data
// behind an Arc that is Send + Sync.
unsafe impl Send for SemaphoreAcquireFuture<'_> {}

// Marker trait impl.
impl UnwindSafe for SemaphoreAcquireFuture<'_> {}
// Marker trait impl.
impl RefUnwindSafe for SemaphoreAcquireFuture<'_> {}

impl<'a> Future for SemaphoreAcquireFuture<'a> {
    type Output = SemaphorePermit<'a>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<SemaphorePermit<'a>> {
        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };

        // SAFETY: The node is pinned (PhantomPinned) and the state
        // field is the mutex this node registers with.
        match unsafe {
            poll_acquire(
                this.state,
                &this.node,
                &mut this.registered,
                this.permits,
                cx.waker(),
            )
        } {
            Poll::Ready(()) => Poll::Ready(SemaphorePermit {
                state: this.state,
                permits: this.permits,
            }),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for SemaphoreAcquireFuture<'_> {
    fn drop(&mut self) {
        if !self.registered {
            // Not yet polled — no cleanup needed.
            debug_assert!(!self.registered, "registered future must not skip cleanup");
            return;
        }

        // SAFETY: The node is pinned (PhantomPinned) and the state
        // field is the mutex this node was registered with.
        unsafe { drop_acquire_wait(self.state, &self.node, self.permits) }
    }
}

// ---------------------------------------------------------------------------
// Debug impls
// ---------------------------------------------------------------------------

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
            .field("registered", &self.registered)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------
// Embedded variant
// ---------------------------------------------------------------------------

/// Embedded-state container for [`Semaphore`].
///
/// Stores the semaphore state inline, avoiding the heap allocation
/// that [`Semaphore::boxed()`] requires. Create the container with
/// [`new()`][Self::new], pin it, then call [`Semaphore::embedded()`]
/// to obtain a [`RawSemaphore`] handle.
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
/// let sem =
///     unsafe { Semaphore::embedded(container.as_ref()) };
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
                state: Mutex::new(SemaphoreState {
                    available: permits,
                    waiters: WaiterList::new(),
                }),
            },
            _pinned: PhantomPinned,
        }
    }
}

/// Handle to an embedded [`Semaphore`].
///
/// Created via [`Semaphore::embedded()`]. The caller is responsible
/// for ensuring the [`EmbeddedSemaphore`] outlives all handles,
/// acquire futures, and permits.
///
/// The API is identical to [`Semaphore`].
#[derive(Clone, Copy)]
pub struct RawSemaphore {
    inner: NonNull<SemaphoreInner>,
}

// Marker trait impl.
// SAFETY: The NonNull<SemaphoreInner> only points to a value whose
// access is serialized by the internal Mutex.
unsafe impl Send for RawSemaphore {}

// Marker trait impl.
// SAFETY: Same as Send — all mutable access is mediated by the
// internal lock.
unsafe impl Sync for RawSemaphore {}

// Marker trait impl.
impl UnwindSafe for RawSemaphore {}
// Marker trait impl.
impl RefUnwindSafe for RawSemaphore {}

impl RawSemaphore {
    fn inner(&self) -> &SemaphoreInner {
        // SAFETY: The caller of `embedded()` guarantees the
        // container outlives this handle.
        unsafe { self.inner.as_ref() }
    }

    /// Returns a future that resolves to a [`RawSemaphorePermit`]
    /// when a single permit is available.
    #[must_use]
    pub fn acquire(&self) -> RawSemaphoreAcquireFuture {
        self.acquire_many(1)
    }

    /// Returns a future that resolves to a [`RawSemaphorePermit`]
    /// holding `permits` permits.
    ///
    /// # Panics
    ///
    /// Panics if `permits` is zero.
    #[must_use]
    pub fn acquire_many(&self, permits: usize) -> RawSemaphoreAcquireFuture {
        assert!(permits > 0, "cannot acquire zero permits");

        RawSemaphoreAcquireFuture {
            inner: self.inner,
            permits,
            node: UnsafeCell::new(WaiterNode::new()),
            registered: false,
            _pinned: PhantomPinned,
        }
    }

    /// Attempts to acquire a single permit without blocking.
    #[must_use]
    // Mutating try_acquire to always return None breaks tests.
    #[cfg_attr(test, mutants::skip)]
    #[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder.
    pub fn try_acquire(&self) -> Option<RawSemaphorePermit> {
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
    pub fn try_acquire_many(&self, permits: usize) -> Option<RawSemaphorePermit> {
        assert!(permits > 0, "cannot acquire zero permits");

        if try_acquire_inner(&self.inner().state, permits) {
            Some(RawSemaphorePermit {
                inner: self.inner,
                permits,
            })
        } else {
            None
        }
    }
}

/// RAII permit returned by [`RawSemaphore::acquire()`] and
/// [`RawSemaphore::try_acquire()`].
///
/// The permit is returned to the semaphore when dropped.
pub struct RawSemaphorePermit {
    inner: NonNull<SemaphoreInner>,
    permits: usize,
}

// Marker trait impl.
// SAFETY: The permit only holds a NonNull to a Mutex-protected value
// and a plain usize. Sending across threads is safe.
unsafe impl Send for RawSemaphorePermit {}

// Marker trait impl.
// SAFETY: Sharing &RawSemaphorePermit gives no mutable access.
unsafe impl Sync for RawSemaphorePermit {}

// Marker trait impl.
impl UnwindSafe for RawSemaphorePermit {}
// Marker trait impl.
impl RefUnwindSafe for RawSemaphorePermit {}

impl Drop for RawSemaphorePermit {
    // Mutating drop to a no-op causes permits to leak.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        // SAFETY: The embedded() contract guarantees the container
        // outlives this permit.
        let inner = unsafe { self.inner.as_ref() };
        release_permits(&inner.state, self.permits);
    }
}

/// Future returned by [`RawSemaphore::acquire()`] and
/// [`RawSemaphore::acquire_many()`].
///
/// Completes with a [`RawSemaphorePermit`] when enough permits are
/// available.
pub struct RawSemaphoreAcquireFuture {
    inner: NonNull<SemaphoreInner>,
    permits: usize,

    // See SemaphoreAcquireFuture for field documentation.
    node: UnsafeCell<WaiterNode>,
    registered: bool,

    _pinned: PhantomPinned,
}

// Marker trait impl.
// SAFETY: Same reasoning as SemaphoreAcquireFuture — all node access
// is protected by the internal Mutex.
unsafe impl Send for RawSemaphoreAcquireFuture {}

// Marker trait impl.
impl UnwindSafe for RawSemaphoreAcquireFuture {}
// Marker trait impl.
impl RefUnwindSafe for RawSemaphoreAcquireFuture {}

impl Future for RawSemaphoreAcquireFuture {
    type Output = RawSemaphorePermit;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> Poll<RawSemaphorePermit> {
        // SAFETY: We only access fields, we do not move self.
        let this = unsafe { self.get_unchecked_mut() };

        // SAFETY: The container outlives this future per the
        // embedded() contract.
        let inner = unsafe { this.inner.as_ref() };
        // SAFETY: The node is pinned (PhantomPinned) and the state
        // is the mutex this node registers with.
        match unsafe {
            poll_acquire(
                &inner.state,
                &this.node,
                &mut this.registered,
                this.permits,
                cx.waker(),
            )
        } {
            Poll::Ready(()) => Poll::Ready(RawSemaphorePermit {
                inner: this.inner,
                permits: this.permits,
            }),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for RawSemaphoreAcquireFuture {
    fn drop(&mut self) {
        if !self.registered {
            // Not yet polled — no cleanup needed.
            debug_assert!(!self.registered, "registered future must not skip cleanup");
            return;
        }

        // SAFETY: The container outlives this future per the
        // embedded() contract.
        let inner = unsafe { self.inner.as_ref() };
        // SAFETY: The node is pinned (PhantomPinned) and the state
        // is the mutex this node was registered with.
        unsafe { drop_acquire_wait(&inner.state, &self.node, self.permits) }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for EmbeddedSemaphore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EmbeddedSemaphore").finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for RawSemaphore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawSemaphore").finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for RawSemaphorePermit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawSemaphorePermit")
            .field("permits", &self.permits)
            .finish_non_exhaustive()
    }
}

#[cfg_attr(coverage_nightly, coverage(off))]
impl fmt::Debug for RawSemaphoreAcquireFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawSemaphoreAcquireFuture")
            .field("permits", &self.permits)
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

    assert_impl_all!(
        Semaphore: Send, Sync, Clone, UnwindSafe, RefUnwindSafe
    );
    assert_impl_all!(
        SemaphorePermit<'static>: Send, Sync, UnwindSafe, RefUnwindSafe
    );
    assert_not_impl_any!(SemaphorePermit<'static>: Clone);
    assert_impl_all!(
        SemaphoreAcquireFuture<'static>: Send, UnwindSafe, RefUnwindSafe
    );
    assert_not_impl_any!(SemaphoreAcquireFuture<'static>: Sync, Unpin);

    assert_impl_all!(
        EmbeddedSemaphore: Send, Sync, UnwindSafe, RefUnwindSafe
    );
    assert_not_impl_any!(EmbeddedSemaphore: Unpin);
    assert_impl_all!(
        RawSemaphore: Send, Sync, Clone, Copy,
        UnwindSafe, RefUnwindSafe
    );
    assert_impl_all!(
        RawSemaphorePermit: Send, Sync,
        UnwindSafe, RefUnwindSafe
    );
    assert_not_impl_any!(RawSemaphorePermit: Clone);
    assert_impl_all!(
        RawSemaphoreAcquireFuture: Send,
        UnwindSafe, RefUnwindSafe
    );
    assert_not_impl_any!(RawSemaphoreAcquireFuture: Sync, Unpin);

    // --- basic functionality ---

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
        let permit = sem.try_acquire_many(3).unwrap();
        assert!(sem.try_acquire_many(3).is_none());
        assert!(sem.try_acquire_many(2).is_some());
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
    #[should_panic]
    fn acquire_zero_panics() {
        let sem = Semaphore::boxed(1);
        let _future = sem.acquire_many(0);
    }

    #[test]
    #[should_panic]
    fn try_acquire_zero_panics() {
        let sem = Semaphore::boxed(1);
        let _permit = sem.try_acquire_many(0);
    }

    // --- async tests ---

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
    fn fifo_ordering() {
        let sem = Semaphore::boxed(1);
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
        let sem = Semaphore::boxed(2);
        let _p1 = sem.try_acquire().unwrap();
        let _p2 = sem.try_acquire().unwrap();

        // f1 wants 2 permits, f2 wants 1.
        let mut f1 = Box::pin(sem.acquire_many(2));
        let mut f2 = Box::pin(sem.acquire());
        let waker = Waker::noop();
        let mut cx = task::Context::from_waker(waker);

        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());

        // Release 1 permit. f1 needs 2, so head-of-line blocking
        // prevents f2 from acquiring even though it only needs 1.
        drop(_p1);
        assert!(f1.as_mut().poll(&mut cx).is_pending());
        assert!(f2.as_mut().poll(&mut cx).is_pending());

        // Release the second permit. Now f1 can proceed.
        drop(_p2);
        let Poll::Ready(p_f1) = f1.as_mut().poll(&mut cx) else {
            panic!("expected Ready for f1")
        };
        // f2 is still waiting because f1 took both permits.
        assert!(f2.as_mut().poll(&mut cx).is_pending());

        drop(p_f1);
        assert!(f2.as_mut().poll(&mut cx).is_ready());
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
        let permit = sem.try_acquire_many(5).unwrap();
        assert!(sem.try_acquire().is_none());
        drop(permit);
        assert!(sem.try_acquire().is_some());
    }

    #[test]
    fn try_acquire_many_exceeds_max() {
        let sem = Semaphore::boxed(3);
        assert!(sem.try_acquire_many(4).is_none());
        // Semaphore is untouched — still has 3 available.
        assert!(sem.try_acquire_many(3).is_some());
    }

    #[test]
    fn acquire_many_all_at_once() {
        futures::executor::block_on(async {
            let sem = Semaphore::boxed(5);
            let permit = sem.acquire_many(5).await;
            assert!(sem.try_acquire().is_none());
            drop(permit);
            assert!(sem.try_acquire_many(5).is_some());
        });
    }

    #[test]
    fn try_acquire_bypasses_waiter_queue() {
        let sem = Semaphore::boxed(3);
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
        let result = std::panic::catch_unwind(move || {
            let _permit = handle.try_acquire().unwrap();
            panic!("intentional");
        });
        assert!(result.is_err());
        // Permit drop during unwinding must return the permit.
        assert!(sem.try_acquire().is_some());
    }

    // --- multithreaded tests (Miri-compatible) ---

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

    // --- embedded variant tests ---

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
