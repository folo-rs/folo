use std::{
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use futures::Stream;
use infinity_pool::{BlindPool, BlindPooledMut};

use crate::{
    deque_future::{DequeFuture, PooledCastDequeFuture as _},
    future_deque_core::{FutureDequeCore, FutureHandle},
};

thread_local! {
    static FUTURES_POOL: BlindPool = BlindPool::new();
}

impl<T> FutureHandle<T> for BlindPooledMut<dyn DequeFuture<T>> {
    fn as_pin_mut(&mut self) -> Pin<&mut dyn DequeFuture<T>> {
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
/// Futures and per-slot waker metadata are stored in thread-local object pools, avoiding
/// per-future heap allocations after pool warm-up. Each future gets its own waker that
/// tracks activation state, so only futures that have been woken since the last poll
/// cycle are re-polled.
///
/// This type requires all inserted futures to be `Send`. For a variant that allows `!Send`
/// futures, see [`LocalFutureDeque`][crate::LocalFutureDeque].
///
/// # Implements `Stream`
///
/// The [`Stream`] implementation drives all active futures and yields completed results
/// from the front of the deque. To retrieve results from the back, use [`pop_back`][Self::pop_back]
/// after driving the deque (e.g. via a `Stream::poll_next` call or by calling it
/// within an async context).
#[non_exhaustive]
pub struct FutureDeque<T> {
    futures_pool: BlindPool,
    core: FutureDequeCore<T, BlindPooledMut<dyn DequeFuture<T>>>,
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
        let handle = handle.cast_deque_future::<T>();
        self.core.push_back_handle(handle);
    }

    /// Adds a future to the front of the deque.
    pub fn push_front(&mut self, future: impl Future<Output = T> + Send + 'static) {
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

impl<T> Stream for FutureDeque<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        self.get_mut().core.poll_next(cx)
    }
}

// SAFETY: All inserted futures are required to be Send (enforced by push method bounds).
// Pool handles (BlindPooledMut) are Send when contents are Send. The shared_parent Arc
// and MetaPtr are Send+Sync. The BlindPool clone is Send+Sync.
unsafe impl<T: Send> Send for FutureDeque<T> {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(FutureDeque<u32>: Send, Sync, Unpin);
}
