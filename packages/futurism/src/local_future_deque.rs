use std::{
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use futures::Stream;
use infinity_pool::{LocalBlindPool, LocalBlindPooledMut};

use crate::{
    deque_future::{DequeFuture, PooledCastDequeFuture as _},
    future_deque_core::{FutureDequeCore, FutureHandle},
};

thread_local! {
    static LOCAL_FUTURES_POOL: LocalBlindPool = LocalBlindPool::new();
}

impl<T> FutureHandle<T> for LocalBlindPooledMut<dyn DequeFuture<T>> {
    fn as_pin_mut(&mut self) -> Pin<&mut dyn DequeFuture<T>> {
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
/// allows results to be popped from either end with strict deque semantics. Futures and
/// per-slot waker metadata are stored in thread-local object pools for efficient memory
/// reuse, and each future gets its own waker for activation tracking.
///
/// # Implements `Stream`
///
/// The [`Stream`] implementation drives all active futures and yields completed results
/// from the front of the deque. To retrieve results from the back, use
/// [`pop_back`][Self::pop_back] after driving the deque.
#[non_exhaustive]
pub struct LocalFutureDeque<T> {
    futures_pool: LocalBlindPool,
    core: FutureDequeCore<T, LocalBlindPooledMut<dyn DequeFuture<T>>>,
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
        let handle = handle.cast_deque_future::<T>();
        self.core.push_back_handle(handle);
    }

    /// Adds a future to the front of the deque.
    pub fn push_front(&mut self, future: impl Future<Output = T> + 'static) {
        let handle = self.futures_pool.insert(future);
        let handle = handle.cast_deque_future::<T>();
        self.core.push_front_handle(handle);
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

impl<T> Stream for LocalFutureDeque<T> {
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
