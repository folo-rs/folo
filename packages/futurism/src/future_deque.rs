use std::{
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use futures::Stream;

use crate::future_deque_core::FutureDequeCore;

/// A deque of futures with deterministic front-to-back polling order.
///
/// `FutureDeque` is a thread-mobile (`Send`) collection of futures that all produce output
/// type `T`. It polls active futures in deterministic front-to-back order and allows results
/// to be popped from either end with strict deque semantics (only the actual front or back
/// item can be popped, and only if it has completed).
///
/// Futures are stored in a pool-backed allocation scheme using
/// [`RawBlindPool`][infinity_pool::RawBlindPool], avoiding per-future heap allocations.
/// Each future gets its own waker that tracks activation state, so only futures that have
/// been woken since the last poll cycle are re-polled.
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
    core: FutureDequeCore<T>,
}

impl<T> FutureDeque<T> {
    /// Creates an empty `FutureDeque`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            core: FutureDequeCore::new(),
        }
    }

    /// Adds a future to the back of the deque.
    pub fn push_back(&mut self, future: impl Future<Output = T> + Send + 'static) {
        self.core.push_back(future);
    }

    /// Adds a future to the front of the deque.
    pub fn push_front(&mut self, future: impl Future<Output = T> + Send + 'static) {
        self.core.push_front(future);
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
            .finish()
    }
}

impl<T> Stream for FutureDeque<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        self.get_mut().core.poll_next(cx)
    }
}

// SAFETY: All inserted futures are required to be Send (enforced by the push method bounds).
// The RawBlindPool is !Send by default as a conservative guard, but its documentation states
// that if all stored items are Send, the owner may treat the pool as Send via unsafe code.
// FutureDeque enforces the Send bound on all inserted futures, satisfying this requirement.
unsafe impl<T: Send> Send for FutureDeque<T> {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(FutureDeque<u32>: Send, Unpin);
    assert_not_impl_any!(FutureDeque<u32>: Sync);
}
