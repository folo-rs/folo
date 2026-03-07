use std::{
    cell::RefCell,
    fmt,
    future::Future,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll},
};

use futures::Stream;
use infinity_pool::{RawBlindPool, RawBlindPooledMut};

use crate::{
    deque_future::{DequeFuture, RawPooledCastDequeFuture as _},
    future_deque_core::{FutureDequeCore, FutureHandle},
};

thread_local! {
    static LOCAL_FUTURES_POOL: Rc<RefCell<RawBlindPool>> =
        Rc::new(RefCell::new(RawBlindPool::new()));
}

/// Wrapper around a raw pool handle that removes the future from the pool on drop.
///
/// Stores an `Rc` clone of the thread-local pool to ensure the pool remains alive
/// and accessible during Drop, even if the thread-local itself has been destroyed.
pub(crate) struct LocalHandle<T> {
    inner: Option<RawBlindPooledMut<dyn DequeFuture<T>>>,
    pool: Rc<RefCell<RawBlindPool>>,
}

impl<T> FutureHandle<T> for LocalHandle<T> {
    fn as_pin_mut(&mut self) -> Pin<&mut dyn DequeFuture<T>> {
        let handle = self
            .inner
            .as_mut()
            .expect("handle is always Some while the LocalHandle exists");

        // SAFETY: The pool is alive (we hold an Rc) and the handle is valid
        // (it has not been removed from the pool).
        unsafe { handle.as_pin_mut() }
    }
}

impl<T> Drop for LocalHandle<T> {
    // Removes the future from the thread-local pool when the handle is dropped.
    // Defense in depth: the pool's own Drop also cleans up remaining entries.
    #[cfg_attr(test, mutants::skip)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn drop(&mut self) {
        if let Some(handle) = self.inner.take() {
            // SAFETY: The handle was inserted into this pool and has not been removed.
            unsafe {
                self.pool.borrow_mut().remove(handle);
            }
        }
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
    futures_pool: Rc<RefCell<RawBlindPool>>,
    core: FutureDequeCore<T, LocalHandle<T>>,
}

impl<T> LocalFutureDeque<T> {
    /// Creates an empty `LocalFutureDeque`.
    #[must_use]
    pub fn new() -> Self {
        Self {
            futures_pool: LOCAL_FUTURES_POOL.with(Rc::clone),
            core: FutureDequeCore::new(),
        }
    }

    /// Adds a future to the back of the deque.
    pub fn push_back(&mut self, future: impl Future<Output = T> + 'static) {
        let handle = self.futures_pool.borrow_mut().insert(future);

        // SAFETY: The concrete type F implements DequeFuture<T>, so the cast is valid.
        let handle = unsafe { handle.cast_deque_future::<T>() };

        self.core.push_back_handle(LocalHandle {
            inner: Some(handle),
            pool: Rc::clone(&self.futures_pool),
        });
    }

    /// Adds a future to the front of the deque.
    pub fn push_front(&mut self, future: impl Future<Output = T> + 'static) {
        let handle = self.futures_pool.borrow_mut().insert(future);

        // SAFETY: The concrete type F implements DequeFuture<T>, so the cast is valid.
        let handle = unsafe { handle.cast_deque_future::<T>() };

        self.core.push_front_handle(LocalHandle {
            inner: Some(handle),
            pool: Rc::clone(&self.futures_pool),
        });
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
