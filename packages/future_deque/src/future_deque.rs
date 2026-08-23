use std::fmt;
use std::future::Future;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::task::{Context, Poll};

use plurality::MultiPool;

use crate::erased_future::alloc_future;
use crate::future_deque_core::FutureDequeCore;

// Each thread allocates from its own pool to avoid synchronization. Handles own their slots,
// so they remain valid after the pool and its thread are gone.
thread_local! {
    static FUTURES_POOL: MultiPool = MultiPool::new();
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
/// popping for convenience. They return `Poll::Ready(Some(value))` when the respective
/// end has a completed future, `Poll::Ready(None)` when the deque is empty, or
/// `Poll::Pending` when the respective end is not yet ready.
#[cfg_attr(
    any(test, feature = "futures-stream"),
    doc = "With the `futures-stream` feature, `FutureDeque` also implements [`Stream`][futures_core::Stream], yielding completed results from the front."
)]
#[cfg_attr(
    not(any(test, feature = "futures-stream")),
    doc = "With the `futures-stream` feature, `FutureDeque` also implements `Stream`, yielding completed results from the front."
)]
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
        let handle = FUTURES_POOL.with(|pool| alloc_future(pool, future));
        self.core.push_back_handle(handle);
    }

    /// Adds a future to the front of the deque.
    pub fn push_front(&mut self, future: impl Future<Output = T> + Send + 'static) {
        let handle = FUTURES_POOL.with(|pool| alloc_future(pool, future));
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
#[cfg_attr(test, mutants::skip)] // Trivial forwarder to core.poll().
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
// `plurality::Box<dyn ErasedFuture<T>>` is not automatically `Send`. However, `push_back`
// and `push_front` both require `F: Future + Send + 'static`, guaranteeing that every value
// behind the trait object is in fact `Send`. Erasure deliberately drops the marker bound so
// that a single erased handle type serves both deque variants. Pool slots can be released
// from any thread. All other state — result values of type `T`, the shared parent waker
// behind `Arc<Mutex<Waker>>`, and the waker metadata reached through `MetaPtr` (itself
// declared `Send`) — is `Send` given `T: Send`.
unsafe impl<T: Send> Send for FutureDeque<T> {}

// SAFETY: `Sync` is likewise blocked only by the erased `dyn ErasedFuture<T>`. Sharing
// `&FutureDeque<T>` never exposes a reference to a stored future or to a stored result: the
// only `&self` operations are `len`, `is_empty` and `Debug`, which read slot counts and
// discriminants. Every state transition — polling, pushing, popping, dropping — requires
// `&mut self` or ownership, so the borrow checker serializes all access to the futures
// themselves. `T: Sync` mirrors the bound the compiler would infer for the result values.
unsafe impl<T: Sync> Sync for FutureDeque<T> {}

// The erased `dyn ErasedFuture<T>` trait object is not `UnwindSafe`, which blocks
// auto-derivation for every type that transitively contains a stored future. The guarantee
// nevertheless holds: all mutable state is either behind `Arc<Mutex<…>>` (unconditionally
// unwind-safe due to poisoning) or confined to owned handles that are never shared through
// references. A `FutureDeque` that survives a panic is safe to drop or continue using
// because each slot independently tracks its own lifecycle.
impl<T> UnwindSafe for FutureDeque<T> {}
impl<T> RefUnwindSafe for FutureDeque<T> {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::task::{Context, Poll, Waker};

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(FutureDeque<u32>: Send, Sync, Unpin, Future,
        UnwindSafe, RefUnwindSafe);

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

    /// The output type carries no `'static` bound, so a deque can be named with a borrowed
    /// element type. Erasing the future behind a trait object is what puts this at risk, so it
    /// is asserted here. The futures themselves capture nothing, which keeps them `'static` as
    /// the push methods require.
    #[test]
    fn push_accepts_borrowed_output_type() {
        // The parameter exists to introduce an `'a` that is provably shorter than `'static`.
        fn drain_borrowed<'a>(_borrow: &'a str) -> Option<&'a str> {
            let mut deque = FutureDeque::<&'a str>::new();
            deque.push_back(async { "back" });
            deque.push_front(async { "front" });

            let waker = Waker::noop();
            let cx = &mut Context::from_waker(waker);

            match deque.poll_front(cx) {
                Poll::Ready(value) => value,
                Poll::Pending => None,
            }
        }

        let owned = "borrowed".to_string();
        assert_eq!(drain_borrowed(&owned), Some("front"));
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
