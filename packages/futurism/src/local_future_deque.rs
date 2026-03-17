use std::{
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use infinity_pool::{LocalBlindPool, LocalBlindPooledMut};

use crate::{
    erased_future::{ErasedFuture, PooledCastErasedFuture as _},
    future_deque_core::{FutureDequeCore, FutureHandle},
};

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
/// # Driving the deque
///
/// Before results can be popped, futures must be driven by calling [`drive`][Self::drive]
/// with a task context. This polls all activated futures and transitions completed ones to
/// ready state.
///
/// [`poll_front`][Self::poll_front] and [`poll_back`][Self::poll_back] combine driving with
/// popping for convenience.
///
/// With the `futures-stream` feature (enabled by default), `LocalFutureDeque` also
/// implements [`Stream`][futures_core::Stream], yielding completed results from the front.
///
/// # Examples
///
/// Using `poll_front` to drive and retrieve results:
///
/// ```rust
/// use std::{
///     pin::Pin,
///     task::{Context, Poll, Waker},
/// };
///
/// use futurism::LocalFutureDeque;
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
/// Manually driving and popping from either end:
///
/// ```rust
/// use std::task::{Context, Waker};
///
/// use futurism::LocalFutureDeque;
///
/// let mut deque = LocalFutureDeque::new();
///
/// deque.push_back(async { 1 });
/// deque.push_back(async { 2 });
/// deque.push_back(async { 3 });
///
/// let waker = Waker::noop();
/// let cx = &mut Context::from_waker(waker);
/// deque.drive(cx);
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

    /// Drives all active futures, polling each activated one front-to-back.
    ///
    /// Futures that complete are transitioned to ready state and can be retrieved
    /// via [`pop_front`][Self::pop_front] or [`pop_back`][Self::pop_back].
    pub fn drive(&mut self, cx: &Context<'_>) {
        self.core.drive(cx);
    }

    /// Drives all active futures and pops the front result if ready.
    ///
    /// Returns `Poll::Ready(Some(value))` if the frontmost future has completed,
    /// `Poll::Ready(None)` if the deque is empty, or `Poll::Pending` if all
    /// remaining futures are still pending.
    pub fn poll_front(&mut self, cx: &Context<'_>) -> Poll<Option<T>> {
        self.core.poll_next(cx)
    }

    /// Drives all active futures and pops the back result if ready.
    ///
    /// Returns `Poll::Ready(Some(value))` if the backmost future has completed,
    /// `Poll::Ready(None)` if the deque is empty, or `Poll::Pending` if all
    /// remaining futures are still pending.
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

#[cfg(feature = "futures-stream")]
#[cfg_attr(docsrs, doc(cfg(feature = "futures-stream")))]
impl<T> futures_core::Stream for LocalFutureDeque<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        self.get_mut().core.poll_next(cx)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::*;

    assert_not_impl_any!(LocalFutureDeque<u32>: Send, Sync);
}
