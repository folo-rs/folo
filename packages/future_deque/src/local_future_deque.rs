use std::fmt;
use std::future::Future;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::task::{Context, Poll};

use infinity_pool::{LocalBlindPool, LocalBlindPooledMut};

use crate::erased_future::{ErasedFuture, PooledCastErasedFuture as _};
use crate::future_deque_core::{FutureDequeCore, FutureHandle};

// Thread-local object pool for storing type-erased futures in the `!Send` variant.
// Uses `LocalBlindPool` (Rc-based) instead of `BlindPool` (Arc-based) because the
// deque and all its handles are confined to a single thread. The pool is cloned
// (Rc reference-counted) into each `LocalFutureDeque` so handles remain valid for
// the lifetime of the deque.
thread_local! {
    static LOCAL_FUTURES_POOL: LocalBlindPool = LocalBlindPool::new();
}

// Bridges the generic `FutureHandle<T>` abstraction to the concrete
// `LocalBlindPooledMut` handle type used by the `!Send` variant.
#[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder to pool handle method.
impl<T> FutureHandle<T> for LocalBlindPooledMut<dyn ErasedFuture<T>> {
    fn as_pin_mut(&mut self) -> Pin<&mut dyn ErasedFuture<T>> {
        LocalBlindPooledMut::as_pin_mut(self)
    }
}

/// A deque of futures with deterministic front-to-back polling order (single-threaded).
///
/// `LocalFutureDeque` is the `!Send` variant of [`FutureDeque`][crate::FutureDeque] that
/// allows inserting futures that are not `Send`. It is strictly single-threaded and cannot
/// be moved between threads.
///
/// Like `FutureDeque`, it polls active futures in deterministic front-to-back order and
/// allows results to be popped from either end with strict deque semantics.
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
/// popping for convenience. They return `Poll::Ready(Some(value))` when the respective
/// end has a completed future, `Poll::Ready(None)` when the deque is empty, or
/// `Poll::Pending` when the respective end is not yet ready.
///
/// With the `futures-stream` feature, `LocalFutureDeque` also
/// implements [`Stream`][futures_core::Stream], yielding completed results from the front.
///
/// # Examples
///
/// Using `poll_front` to poll and retrieve results:
///
/// ```rust
/// use std::task::{Context, Poll, Waker};
///
/// use future_deque::LocalFutureDeque;
///
/// let mut deque = LocalFutureDeque::new();
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
/// use future_deque::LocalFutureDeque;
///
/// let mut deque = LocalFutureDeque::new();
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
pub struct LocalFutureDeque<T> {
    futures_pool: LocalBlindPool,
    core: FutureDequeCore<T, LocalBlindPooledMut<dyn ErasedFuture<T>>>,
}

impl<T> LocalFutureDeque<T> {
    /// Creates an empty `LocalFutureDeque`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            futures_pool: LOCAL_FUTURES_POOL.with(LocalBlindPool::clone),
            core: FutureDequeCore::new(),
        }
    }

    /// Adds a future to the back of the deque.
    pub fn push_back(&mut self, future: impl Future<Output = T> + 'static) {
        let handle = self.futures_pool.insert(future);
        let handle = handle.cast_erased_future::<T>();
        self.core.push_back_handle(handle);
    }

    /// Adds a future to the front of the deque.
    pub fn push_front(&mut self, future: impl Future<Output = T> + 'static) {
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

impl<T> Default for LocalFutureDeque<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> fmt::Debug for LocalFutureDeque<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalFutureDeque")
            .field("len", &self.core.len())
            .finish_non_exhaustive()
    }
}

/// `LocalFutureDeque` implements [`Future`] to allow waiting for all contained futures to
/// complete. `Poll::Ready(())` indicates that every future in the deque has finished
/// (or the deque is empty). The deque may be polled again after returning `Ready`, for
/// example after pushing new futures.
#[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder to core.poll().
#[cfg_attr(test, mutants::skip)] // Trivial forwarder to core.poll().
impl<T> Future for LocalFutureDeque<T> {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.get_mut().core.poll(cx)
    }
}

#[cfg(any(test, feature = "futures-stream"))]
#[cfg_attr(coverage_nightly, coverage(off))] // Trivial forwarder to core.poll_next().
impl<T> futures_core::Stream for LocalFutureDeque<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        self.get_mut().core.poll_next(cx)
    }
}

// The internal `dyn ErasedFuture<T>` trait objects and the `UnsafeCell` inside the waker metadata
// prevent auto-derivation. However, all mutable state is either behind `Mutex` (unconditionally
// unwind-safe due to poisoning) or confined to owned pool handles that are not shared through
// references. A `LocalFutureDeque` that survives a panic is safe to drop or continue using
// because each slot independently tracks its own lifecycle.
impl<T> UnwindSafe for LocalFutureDeque<T> {}
impl<T> RefUnwindSafe for LocalFutureDeque<T> {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::task::{Context, Poll, Waker};

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_not_impl_any!(LocalFutureDeque<u32>: Send, Sync);
    assert_impl_all!(LocalFutureDeque<u32>: Unpin, Future,
        UnwindSafe, RefUnwindSafe);

    #[test]
    fn poll_then_pop_both_ends() {
        let mut deque = LocalFutureDeque::new();
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
        let mut deque = LocalFutureDeque::new();
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
    fn poll_front_ordering() {
        let mut deque = LocalFutureDeque::new();
        deque.push_back(async { 1 });
        deque.push_back(async { 2 });

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        assert_eq!(deque.poll_front(cx), Poll::Ready(Some(1)));
        assert_eq!(deque.poll_front(cx), Poll::Ready(Some(2)));
        assert_eq!(deque.poll_front(cx), Poll::Ready(None));
    }

    #[test]
    fn default_creates_empty() {
        let deque: LocalFutureDeque<i32> = LocalFutureDeque::default();
        assert!(deque.is_empty());
    }

    #[test]
    fn debug_output() {
        let mut deque = LocalFutureDeque::new();
        deque.push_back(async { 1 });
        let debug = format!("{deque:?}");
        assert!(debug.contains("LocalFutureDeque"));
        assert!(debug.contains("len: 1"));
    }
}
