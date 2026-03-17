use std::{
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use infinity_pool::{BlindPool, BlindPooledMut};

use crate::{
    erased_future::{ErasedFuture, PooledCastErasedFuture as _},
    future_deque_core::{FutureDequeCore, FutureHandle},
};

// Thread-local object pool for storing type-erased futures. Each thread gets its own pool
// instance, so futures inserted on one thread are backed by that thread's slab allocator.
// The pool is cloned (reference-counted) into each `FutureDeque` instance so that pool
// handles remain valid for the lifetime of the deque, even if the thread-local is destroyed
// first.
thread_local! {
    static FUTURES_POOL: BlindPool = BlindPool::new();
}

// Bridges the generic `FutureHandle<T>` abstraction to the concrete `BlindPooledMut` handle
// type used by the `Send` variant. The `LocalFutureDeque` has an equivalent impl for
// `LocalBlindPooledMut`.
#[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder to pool handle method.
impl<T> FutureHandle<T> for BlindPooledMut<dyn ErasedFuture<T>> {
    fn as_pin_mut(&mut self) -> Pin<&mut dyn ErasedFuture<T>> {
        BlindPooledMut::as_pin_mut(self)
    }
}

/// A deque of futures with deterministic front-to-back polling order.
///
/// `FutureDeque` is a thread-mobile (`Send`) collection of futures that all produce output
/// type `T`. It polls active futures in deterministic front-to-back order and allows results
/// to be popped from either end with strict deque semantics (only the actual front or back
/// item can be popped, and only if it has completed).
///
/// This type requires all inserted futures to be `Send`. For a variant that allows `!Send`
/// futures, see [`LocalFutureDeque`][crate::LocalFutureDeque].
///
/// # Polling the deque
///
/// Before results can be popped, futures must be polled by calling [`poll`][Self::poll]
/// with a task context. This polls all activated futures and transitions completed ones to
/// ready state. Returns `Poll::Ready(())` when all contained futures have completed (or
/// the deque is empty), and `Poll::Pending` while any futures remain pending. Pushing new
/// futures resets readiness back to pending.
///
/// The deque also implements [`Future`], allowing `.await` to wait for all contained
/// futures to complete. The deque may be polled again after returning `Poll::Ready(())`
/// (e.g. after pushing new futures).
///
/// [`poll_front`][Self::poll_front] and [`poll_back`][Self::poll_back] combine polling with
/// popping for convenience.
///
/// With the `futures-stream` feature (enabled by default), `FutureDeque` also implements
/// [`Stream`][futures_core::Stream], yielding completed results from the front.
///
/// # Examples
///
/// Using `poll_front` to poll and retrieve results:
///
/// ```rust
/// use std::task::{Context, Poll, Waker};
///
/// use future_deque::FutureDeque;
///
/// let mut deque = FutureDeque::new();
///
/// deque.push_back(async { 10 });
/// deque.push_back(async { 20 });
///
/// let waker = Waker::noop();
/// let cx = &mut Context::from_waker(waker);
///
/// assert_eq!(deque.poll_front(cx), Poll::Ready(Some(10)));
/// assert_eq!(deque.poll_front(cx), Poll::Ready(Some(20)));
/// assert_eq!(deque.poll_front(cx), Poll::Ready(None));
/// ```
///
/// Polling all futures to completion, then popping results:
///
/// ```rust
/// use std::task::{Context, Poll, Waker};
///
/// use future_deque::FutureDeque;
///
/// let mut deque = FutureDeque::new();
///
/// deque.push_back(async { 1 });
/// deque.push_back(async { 2 });
/// deque.push_back(async { 3 });
///
/// let waker = Waker::noop();
/// let cx = &mut Context::from_waker(waker);
/// assert_eq!(deque.poll(cx), Poll::Ready(()));
///
/// assert_eq!(deque.pop_back(), Some(3));
/// assert_eq!(deque.pop_front(), Some(1));
/// assert_eq!(deque.pop_front(), Some(2));
/// ```
pub struct FutureDeque<T> {
    futures_pool: BlindPool,
    core: FutureDequeCore<T, BlindPooledMut<dyn ErasedFuture<T>>>,
}

impl<T> FutureDeque<T> {
    /// Creates an empty `FutureDeque`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            futures_pool: FUTURES_POOL.with(BlindPool::clone),
            core: FutureDequeCore::new(),
        }
    }

    /// Adds a future to the back of the deque.
    pub fn push_back(&mut self, future: impl Future<Output = T> + Send + 'static) {
        let handle = self.futures_pool.insert(future);
        let handle = handle.cast_erased_future::<T>();
        self.core.push_back_handle(handle);
    }

    /// Adds a future to the front of the deque.
    pub fn push_front(&mut self, future: impl Future<Output = T> + Send + 'static) {
        let handle = self.futures_pool.insert(future);
        let handle = handle.cast_erased_future::<T>();
        self.core.push_front_handle(handle);
    }

    /// Polls all active futures, polling each activated one front-to-back.
    ///
    /// Futures that complete are transitioned to ready state and can be retrieved
    /// via [`pop_front`][Self::pop_front] or [`pop_back`][Self::pop_back].
    ///
    /// Returns `Poll::Ready(())` when no pending futures remain, `Poll::Pending` otherwise.
    pub fn poll(&mut self, cx: &Context<'_>) -> Poll<()> {
        self.core.poll(cx)
    }

    /// Polls all active futures and pops the front result if ready.
    ///
    /// Returns `Poll::Ready(Some(value))` if the frontmost future has completed,
    /// `Poll::Ready(None)` if the deque is empty, or `Poll::Pending` if the front
    /// future has not yet completed. Note that `Ready(None)` means the deque has no
    /// entries at all, unlike [`poll`][Self::poll] which returns `Ready(())` when all
    /// entries have finished but may still contain poppable results.
    pub fn poll_front(&mut self, cx: &Context<'_>) -> Poll<Option<T>> {
        self.core.poll_next(cx)
    }

    /// Polls all active futures and pops the back result if ready.
    ///
    /// Returns `Poll::Ready(Some(value))` if the backmost future has completed,
    /// `Poll::Ready(None)` if the deque is empty, or `Poll::Pending` if the back
    /// future has not yet completed. See [`poll_front`][Self::poll_front] for details
    /// on how this differs from [`poll`][Self::poll].
    pub fn poll_back(&mut self, cx: &Context<'_>) -> Poll<Option<T>> {
        self.core.poll_back(cx)
    }

    /// Pops the front result if the frontmost future has completed.
    ///
    /// Returns `None` if the deque is empty or the front future is still pending.
    #[must_use]
    pub fn pop_front(&mut self) -> Option<T> {
        self.core.pop_front()
    }

    /// Pops the back result if the backmost future has completed.
    ///
    /// Returns `None` if the deque is empty or the back future is still pending.
    #[must_use]
    pub fn pop_back(&mut self) -> Option<T> {
        self.core.pop_back()
    }

    /// Returns the number of entries (both pending and completed) in the deque.
    #[must_use]
    pub fn len(&self) -> usize {
        self.core.len()
    }

    /// Returns `true` if the deque contains no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.core.is_empty()
    }
}

impl<T> Default for FutureDeque<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> fmt::Debug for FutureDeque<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FutureDeque")
            .field("len", &self.core.len())
            .finish_non_exhaustive()
    }
}

/// `FutureDeque` implements [`Future`] to allow waiting for all contained futures to
/// complete. `Poll::Ready(())` indicates that every future in the deque has finished
/// (or the deque is empty). The deque may be polled again after returning `Ready`, for
/// example after pushing new futures.
#[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder to core.poll().
impl<T> Future for FutureDeque<T> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.get_mut().core.poll(cx)
    }
}

#[cfg(any(test, feature = "futures-stream"))]
#[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder to core.poll_next().
impl<T> futures_core::Stream for FutureDeque<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        self.get_mut().core.poll_next(cx)
    }
}

// SAFETY: The erased type `dyn ErasedFuture<T>` does not carry a `Send` bound, so
// `BlindPooledMut<dyn ErasedFuture<T>>` is not automatically `Send`. However, `push_back`
// and `push_front` both require `F: Future + Send + 'static`, guaranteeing that every
// value behind the trait object is in fact `Send`. This is the intended usage pattern of
// `BlindPooledMut` — it deliberately does not require `T: Send` so that trait object
// casts do not need to carry marker bounds. All other fields (`BlindPool`, waker metadata
// behind `Arc<Mutex<…>>`) are `Send + Sync`.
unsafe impl<T: Send> Send for FutureDeque<T> {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::task::{Context, Poll, Waker};

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(FutureDeque<u32>: Send, Sync, Unpin, Future);

    #[test]
    fn push_back_and_poll_front() {
        let mut deque = FutureDeque::new();
        deque.push_back(async { 1 });
        deque.push_back(async { 2 });

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        assert_eq!(deque.poll_front(cx), Poll::Ready(Some(1)));
        assert_eq!(deque.poll_front(cx), Poll::Ready(Some(2)));
        assert_eq!(deque.poll_front(cx), Poll::Ready(None));
    }

    #[test]
    fn push_front_ordering() {
        let mut deque = FutureDeque::new();
        deque.push_back(async { 2 });
        deque.push_front(async { 1 });

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        assert_eq!(deque.poll_front(cx), Poll::Ready(Some(1)));
        assert_eq!(deque.poll_front(cx), Poll::Ready(Some(2)));
    }

    #[test]
    fn poll_then_pop_both_ends() {
        let mut deque = FutureDeque::new();
        deque.push_back(async { 10 });
        deque.push_back(async { 20 });
        deque.push_back(async { 30 });

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        assert_eq!(deque.poll(cx), Poll::Ready(()));

        assert_eq!(deque.pop_back(), Some(30));
        assert_eq!(deque.pop_front(), Some(10));
        assert_eq!(deque.pop_front(), Some(20));
        assert!(deque.is_empty());
    }

    #[test]
    fn poll_back_returns_last_ready() {
        let mut deque = FutureDeque::new();
        deque.push_back(async { 1 });
        deque.push_back(async { 2 });
        deque.push_back(async { 3 });

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        assert_eq!(deque.poll_back(cx), Poll::Ready(Some(3)));
        assert_eq!(deque.poll_back(cx), Poll::Ready(Some(2)));
        assert_eq!(deque.poll_back(cx), Poll::Ready(Some(1)));
        assert_eq!(deque.poll_back(cx), Poll::Ready(None));
    }

    #[test]
    fn len_and_is_empty() {
        let mut deque = FutureDeque::new();
        assert!(deque.is_empty());
        assert_eq!(deque.len(), 0);

        deque.push_back(async { 1 });
        assert!(!deque.is_empty());
        assert_eq!(deque.len(), 1);

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);
        assert_eq!(deque.poll_front(cx), Poll::Ready(Some(1)));
        assert!(deque.is_empty());
    }

    #[test]
    fn default_creates_empty() {
        let deque: FutureDeque<i32> = FutureDeque::default();
        assert!(deque.is_empty());
    }

    #[test]
    fn debug_output() {
        let mut deque = FutureDeque::new();
        deque.push_back(async { 1 });
        let debug = format!("{deque:?}");
        assert!(debug.contains("FutureDeque"));
        assert!(debug.contains("len: 1"));
    }
}
