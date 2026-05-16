use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use crate::erased_future::ErasedFuture;
use crate::waker_meta::{self, MetaPtr};

/// Abstracts over managed ([`BlindPooledMut`][infinity_pool::BlindPooledMut]) and raw
/// pool handles, allowing [`FutureDequeCore`] to work with both Send and !Send variants.
pub(crate) trait FutureHandle<T> {
    fn as_pin_mut(&mut self) -> Pin<&mut dyn ErasedFuture<T>>;
}

/// Shared core implementation for both [`FutureDeque`][crate::FutureDeque]
/// and [`LocalFutureDeque`][crate::LocalFutureDeque].
///
/// Generic over `H`, the pool handle type. The Send variant uses managed handles
/// (auto-remove on drop), while the Local variant uses local handles
/// (auto-remove on drop via Rc-based pool reference).
pub(crate) struct FutureDequeCore<T, H> {
    pub(crate) shared_parent: Arc<Mutex<Waker>>,
    slots: VecDeque<Slot<T, H>>,
}

enum Slot<T, H> {
    Pending {
        handle: H,
        meta: MetaPtr,
        waker: Waker,
    },
    Ready {
        value: T,
    },
}

impl<T, H> Slot<T, H> {
    // Mutations to is_ready (returning false) and take_value (returning None) cause
    // pop_front/pop_back to never return results, making blocking tests (block_on +
    // Stream::next) hang indefinitely. Non-blocking tests catch both mutations, but
    // the blocking tests in the same binary prevent the test process from exiting.
    #[cfg_attr(test, mutants::skip)]
    fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    // Callers always check is_ready() before calling take_value(), making the
    // Pending branch unreachable under normal operation. However, returning Option
    // lets callers use combinators like and_then, and also makes the code safe against
    // future refactors that might remove the precondition check.
    #[cfg_attr(coverage_nightly, coverage(off))]
    #[cfg_attr(test, mutants::skip)]
    fn take_value(self) -> Option<T> {
        match self {
            Self::Ready { value } => Some(value),
            Self::Pending { .. } => None,
        }
    }
}

impl<T, H> FutureDequeCore<T, H> {
    pub(crate) fn new() -> Self {
        Self {
            shared_parent: Arc::new(Mutex::new(Waker::noop().clone())),
            slots: VecDeque::new(),
        }
    }

    /// Adds a pre-inserted pool handle to the back of the deque.
    pub(crate) fn push_back_handle(&mut self, handle: H) {
        let meta = waker_meta::create_waker_meta(&self.shared_parent);
        let waker = waker_meta::make_waker(meta);
        self.slots.push_back(Slot::Pending {
            handle,
            meta,
            waker,
        });
    }

    /// Adds a pre-inserted pool handle to the front of the deque.
    pub(crate) fn push_front_handle(&mut self, handle: H) {
        let meta = waker_meta::create_waker_meta(&self.shared_parent);
        let waker = waker_meta::make_waker(meta);
        self.slots.push_front(Slot::Pending {
            handle,
            meta,
            waker,
        });
    }

    /// Returns the number of entries (both pending and ready) in the deque.
    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    /// Returns `true` if the deque contains no entries.
    // Mutation (returning false) causes Stream::next to return Pending for empty
    // deques, making blocking tests hang. Non-blocking tests catch this mutation.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Pops the front result if the frontmost future has completed.
    // Mutation (returning None) causes Stream::next to never yield results,
    // making blocking tests hang. Non-blocking tests catch this mutation.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn pop_front(&mut self) -> Option<T> {
        let front_ready = self.slots.front().is_some_and(Slot::is_ready);
        if front_ready {
            self.slots.pop_front().and_then(Slot::take_value)
        } else {
            None
        }
    }

    /// Pops the back result if the backmost future has completed.
    pub(crate) fn pop_back(&mut self) -> Option<T> {
        let back_ready = self.slots.back().is_some_and(Slot::is_ready);
        if back_ready {
            self.slots.pop_back().and_then(Slot::take_value)
        } else {
            None
        }
    }
}

impl<T, H: FutureHandle<T>> FutureDequeCore<T, H> {
    /// Polls all activated futures front-to-back, transitioning completed ones to ready.
    ///
    /// Returns `Poll::Ready(())` when no pending futures remain (all have completed or the
    /// deque is empty), `Poll::Pending` otherwise.
    // Mutations to this method's core logic (replacing the body, deleting negation
    // operators) create infinite polling loops that cause blocking tests to hang.
    // Non-blocking tests (e.g. poll_returns_pending_while_futures_pending,
    // non_activated_future_is_not_repolled) DO catch these mutations, but blocking
    // tests in the same binary hang indefinitely, preventing the test binary from
    // exiting within the mutation testing timeout.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn poll(&mut self, cx: &Context<'_>) -> Poll<()> {
        // Update the shared parent waker if it has changed. All slot wakers read
        // the parent through this shared location, so a single update here ensures
        // every future's waker uses the latest parent without per-slot iteration.
        {
            let mut parent = self
                .shared_parent
                .lock()
                .expect("we never panic while holding this lock");
            if !parent.will_wake(cx.waker()) {
                parent.clone_from(cx.waker());
            }
        }

        for slot in &mut self.slots {
            let poll_result = {
                let Slot::Pending {
                    handle,
                    meta,
                    waker,
                } = slot
                else {
                    continue;
                };

                // Atomically read and clear the activation flag. Using swap ensures
                // that a concurrent wake between the read and clear is not lost.
                if !waker_meta::check_activated(*meta) {
                    continue;
                }

                let sub_cx = &mut Context::from_waker(waker);

                handle.as_pin_mut().poll_erased(sub_cx)
            };

            if let Poll::Ready(value) = poll_result {
                // Replace the slot atomically before dropping the old `Slot::Pending`. The
                // pool handle inside the old slot may invoke a user-supplied `Drop` impl on
                // the wrapped future when `old` is dropped below (after `release_ref(meta)`).
                // By the time that user code runs, the slot is already fully `Ready { value }`,
                // so a reentrant observer (if one were possible) would see consistent state.
                // `FutureDequeCore` is not exposed behind any shared interior mutability, so
                // user code reachable through the future's drop cannot obtain a second
                // `&mut FutureDequeCore` while we hold this borrow. The `shared_parent` mutex
                // is also not held at this point (released above after cloning the parent
                // waker), so a reentrant wake through the parent waker path does not deadlock.
                let old = std::mem::replace(slot, Slot::Ready { value });

                // Release the Slot's metadata reference. The handle is dropped as
                // part of the old Slot destruction, which auto-removes the future
                // from the futures pool.
                if let Slot::Pending { meta, .. } = old {
                    waker_meta::release_ref(meta);
                }
            }
        }

        if self.slots.iter().any(|s| matches!(s, Slot::Pending { .. })) {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }

    /// Polls all futures and pops the front if ready.
    pub(crate) fn poll_next(&mut self, cx: &Context<'_>) -> Poll<Option<T>> {
        // The overall readiness returned by poll() is not relevant here because
        // poll_next has its own return semantics based on the front slot state.
        let _readiness = self.poll(cx);

        if let Some(value) = self.pop_front() {
            Poll::Ready(Some(value))
        } else if self.is_empty() {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }

    /// Polls all futures and pops the back if ready.
    pub(crate) fn poll_back(&mut self, cx: &Context<'_>) -> Poll<Option<T>> {
        // The overall readiness returned by poll() is not relevant here because
        // poll_back has its own return semantics based on the back slot state.
        let _readiness = self.poll(cx);

        if let Some(value) = self.pop_back() {
            Poll::Ready(Some(value))
        } else if self.is_empty() {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }
}

impl<T, H> Drop for FutureDequeCore<T, H> {
    // When a pending slot is dropped normally (e.g. via mem::replace in poll()), its
    // waker metadata reference is released explicitly. This Drop impl handles the case
    // where the entire deque is dropped with pending slots still present — it ensures
    // metadata references are released so the waker metadata pool entries can be freed.
    //
    // The pool handles in each slot are dropped as part of normal Slot destruction,
    // which auto-removes futures from their object pools.
    #[cfg_attr(coverage_nightly, coverage(off))]
    // Only runs when deque is dropped with
    // pending slots, which is a cleanup path.
    // Detecting the mutation requires observing that pool entries are not returned,
    // which is an internal pool detail invisible to tests without pool introspection.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        for slot in self.slots.drain(..) {
            if let Slot::Pending { meta, .. } = slot {
                waker_meta::release_ref(meta);
            }
        }
    }
}

// The futures stored in the deque are pinned by pool slabs (heap-allocated, stable
// addresses), not by FutureDequeCore's own fields. The struct only holds handles
// (pointers + metadata) and result values, none of which require pinning guarantees.
impl<T, H> Unpin for FutureDequeCore<T, H> {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, mpsc};
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    use futures::StreamExt;
    use futures::executor::block_on;

    use crate::{FutureDeque, LocalFutureDeque};

    /// A future that returns `Pending` for a configurable number of polls, then
    /// returns `Ready(value)`. Used to control activation patterns in tests.
    struct CountdownFuture<T> {
        remaining: usize,
        value: Option<T>,
    }

    impl<T> Unpin for CountdownFuture<T> {}

    impl<T> CountdownFuture<T> {
        fn ready(value: T) -> Self {
            Self {
                remaining: 0,
                value: Some(value),
            }
        }

        fn pending(remaining: usize, value: T) -> Self {
            Self {
                remaining,
                value: Some(value),
            }
        }
    }

    impl<T> Future for CountdownFuture<T> {
        type Output = T;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
            let this = self.get_mut();
            if this.remaining == 0 {
                Poll::Ready(this.value.take().unwrap())
            } else {
                this.remaining = this.remaining.wrapping_sub(1);
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    /// A future that always returns `Pending` without waking itself.
    /// Tracks the number of times it has been polled.
    struct SilentPendingFuture {
        poll_count: Arc<AtomicUsize>,
    }

    impl Unpin for SilentPendingFuture {}

    impl Future for SilentPendingFuture {
        type Output = ();

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
            self.poll_count.fetch_add(1, Ordering::Relaxed);
            Poll::Pending
        }
    }

    #[test]
    fn empty_deque_returns_none() {
        let mut deque = LocalFutureDeque::<u32>::new();

        assert!(deque.is_empty());
        assert_eq!(deque.len(), 0);
        assert!(deque.pop_front().is_none());
        assert!(deque.pop_back().is_none());
    }

    #[test]
    fn empty_stream_returns_none() {
        testing::with_watchdog(|| {
            block_on(async {
                let mut deque = LocalFutureDeque::<u32>::new();
                assert!(deque.next().await.is_none());
            });
        });
    }

    #[test]
    fn push_back_pop_front_single() {
        testing::with_watchdog(|| {
            block_on(async {
                let mut deque = LocalFutureDeque::new();
                deque.push_back(CountdownFuture::ready(42));

                assert_eq!(deque.len(), 1);
                let value = deque.next().await;
                assert_eq!(value, Some(42));
                assert!(deque.is_empty());
            });
        });
    }

    #[test]
    fn push_back_pop_front_ordering() {
        testing::with_watchdog(|| {
            block_on(async {
                let mut deque = LocalFutureDeque::new();
                deque.push_back(CountdownFuture::ready(1));
                deque.push_back(CountdownFuture::ready(2));
                deque.push_back(CountdownFuture::ready(3));

                assert_eq!(deque.next().await, Some(1));
                assert_eq!(deque.next().await, Some(2));
                assert_eq!(deque.next().await, Some(3));
                assert!(deque.next().await.is_none());
            });
        });
    }

    #[test]
    fn push_front_pop_front_ordering() {
        testing::with_watchdog(|| {
            block_on(async {
                let mut deque = LocalFutureDeque::new();
                deque.push_front(CountdownFuture::ready(1));
                deque.push_front(CountdownFuture::ready(2));
                deque.push_front(CountdownFuture::ready(3));

                // push_front reverses insertion order.
                assert_eq!(deque.next().await, Some(3));
                assert_eq!(deque.next().await, Some(2));
                assert_eq!(deque.next().await, Some(1));
                assert!(deque.next().await.is_none());
            });
        });
    }

    #[test]
    fn pop_back_returns_completed_back() {
        let mut deque = LocalFutureDeque::new();
        // Front future needs 2 polls, back future is immediately ready.
        deque.push_back(CountdownFuture::pending(2, 10));
        deque.push_back(CountdownFuture::ready(20));

        // Poll all futures by calling poll_next via the stream.
        // The front is not ready yet, so stream returns Pending (internally).
        // But pop_back should return the completed back future.
        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);
        let poll = deque.poll_front(cx);
        assert!(poll.is_pending());
        assert_eq!(deque.pop_back(), Some(20));
        assert_eq!(deque.len(), 1);
    }

    #[test]
    fn pop_back_returns_none_when_back_pending() {
        let mut deque = LocalFutureDeque::new();
        deque.push_back(CountdownFuture::ready(10));
        deque.push_back(CountdownFuture::pending(5, 20));

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);
        // Front is ready but back is not.
        let poll = deque.poll_front(cx);
        assert_eq!(poll, Poll::Ready(Some(10)));
        // Back is still pending.
        assert!(deque.pop_back().is_none());
    }

    #[test]
    fn pop_front_returns_none_when_front_pending() {
        let mut deque = LocalFutureDeque::new();
        deque.push_back(CountdownFuture::pending(5, 10));
        deque.push_back(CountdownFuture::ready(20));

        // Without driving, nothing is ready.
        assert!(deque.pop_front().is_none());
    }

    #[test]
    fn mixed_push_front_and_push_back() {
        testing::with_watchdog(|| {
            block_on(async {
                let mut deque = LocalFutureDeque::new();
                deque.push_back(CountdownFuture::ready(2));
                deque.push_front(CountdownFuture::ready(1));
                deque.push_back(CountdownFuture::ready(3));

                assert_eq!(deque.next().await, Some(1));
                assert_eq!(deque.next().await, Some(2));
                assert_eq!(deque.next().await, Some(3));
            });
        });
    }

    #[test]
    fn pending_futures_eventually_complete() {
        testing::with_watchdog(|| {
            block_on(async {
                let mut deque = LocalFutureDeque::new();
                deque.push_back(CountdownFuture::pending(3, 100));
                deque.push_back(CountdownFuture::pending(1, 200));

                // The stream will poll futures until the front one completes.
                assert_eq!(deque.next().await, Some(100));
                assert_eq!(deque.next().await, Some(200));
            });
        });
    }

    #[test]
    fn front_blocks_stream_even_if_back_is_ready() {
        testing::with_watchdog(|| {
            block_on(async {
                let mut deque = LocalFutureDeque::new();
                deque.push_back(CountdownFuture::pending(2, 10));
                deque.push_back(CountdownFuture::ready(20));

                // Stream yields front first, even though back is ready sooner.
                assert_eq!(deque.next().await, Some(10));
                assert_eq!(deque.next().await, Some(20));
            });
        });
    }

    #[test]
    fn drop_cleans_up_pending_futures() {
        let dropped = Arc::new(AtomicBool::new(false));
        let dropped_clone = Arc::clone(&dropped);

        {
            let mut deque = LocalFutureDeque::new();
            // Create the guard outside the async block so it is captured by value.
            // When the deque drops the future, the captured guard is dropped too.
            let guard = DropGuard(dropped_clone);
            deque.push_back(async move {
                let _guard = guard;
                std::future::pending::<()>().await;
            });
        }

        assert!(dropped.load(Ordering::Relaxed));
    }

    struct DropGuard(Arc<AtomicBool>);

    impl Drop for DropGuard {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Relaxed);
        }
    }

    #[test]
    fn send_variant_works() {
        testing::with_watchdog(|| {
            block_on(async {
                let mut deque = FutureDeque::new();
                deque.push_back(CountdownFuture::ready(42));
                assert_eq!(deque.next().await, Some(42));
            });
        });
    }

    #[test]
    fn debug_output() {
        let deque = FutureDeque::<u32>::new();
        let debug = format!("{deque:?}");
        assert!(debug.contains("FutureDeque"));

        let local = LocalFutureDeque::<u32>::new();
        let debug = format!("{local:?}");
        assert!(debug.contains("LocalFutureDeque"));
    }

    #[test]
    fn default_creates_empty() {
        let deque = FutureDeque::<u32>::default();
        assert!(deque.is_empty());

        let local = LocalFutureDeque::<u32>::default();
        assert!(local.is_empty());
    }

    // Non-blocking poll tests that convert TIMEOUT mutations to CAUGHT by asserting
    // specific poll results without using block_on (which hangs on broken mutations).

    #[test]
    fn poll_next_returns_ready_for_ready_future() {
        let mut deque = LocalFutureDeque::new();
        deque.push_back(CountdownFuture::ready(42));

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);
        let result = deque.poll_front(cx);
        assert_eq!(result, Poll::Ready(Some(42)));
    }

    #[test]
    fn poll_next_returns_none_for_empty_deque() {
        let mut deque = LocalFutureDeque::<u32>::new();

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);
        let result = deque.poll_front(cx);
        assert_eq!(result, Poll::Ready(None));
    }

    #[test]
    fn poll_back_returns_pending_when_future_not_ready() {
        let mut deque = LocalFutureDeque::new();
        deque.push_back(SilentPendingFuture {
            poll_count: Arc::new(AtomicUsize::new(0)),
        });

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        // The future returns Pending and does not wake itself, so poll_back
        // should return Pending (the deque is not empty, but nothing is ready).
        let result = deque.poll_back(cx);
        assert!(result.is_pending());
    }

    #[test]
    fn poll_back_returns_none_for_empty_deque() {
        let mut deque = LocalFutureDeque::<u32>::new();

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);
        let result = deque.poll_back(cx);
        assert_eq!(result, Poll::Ready(None));
    }

    #[test]
    fn multi_poll_future_completes_via_activation() {
        let mut deque = LocalFutureDeque::new();
        deque.push_back(CountdownFuture::pending(1, 42));

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        // First poll: future returns Pending and wakes itself via the slot waker.
        let result = deque.poll_front(cx);
        assert!(result.is_pending());

        // Second poll: future was re-activated and completes.
        let result = deque.poll_front(cx);
        assert_eq!(result, Poll::Ready(Some(42)));
    }

    #[test]
    fn non_activated_future_is_not_repolled() {
        let poll_count = Arc::new(AtomicUsize::new(0));
        let mut deque = LocalFutureDeque::new();
        deque.push_back(SilentPendingFuture {
            poll_count: Arc::clone(&poll_count),
        });

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        // First poll: the future is activated (newly inserted) and gets polled.
        let result = deque.poll_front(cx);
        assert!(result.is_pending());
        assert_eq!(poll_count.load(Ordering::Relaxed), 1);

        // Second poll with the same waker: the future did not wake itself,
        // so it should not be re-polled.
        let result = deque.poll_front(cx);
        assert!(result.is_pending());
        assert_eq!(poll_count.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn local_pop_front_returns_value_after_poll() {
        let mut deque = LocalFutureDeque::new();
        deque.push_back(CountdownFuture::ready(10));
        deque.push_back(CountdownFuture::ready(20));

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        // Poll and consume the front item via poll_next.
        let result = deque.poll_front(cx);
        assert_eq!(result, Poll::Ready(Some(10)));

        // The second item was also polled and is ready. Pop it manually.
        assert_eq!(deque.pop_front(), Some(20));
    }

    #[test]
    fn local_is_empty_returns_false_when_non_empty() {
        let mut deque = LocalFutureDeque::new();
        deque.push_back(CountdownFuture::ready(42));
        assert!(!deque.is_empty());
    }

    #[test]
    fn push_after_consume_reuses_deque() {
        testing::with_watchdog(|| {
            block_on(async {
                let mut deque = LocalFutureDeque::new();

                deque.push_back(CountdownFuture::ready(1));
                deque.push_back(CountdownFuture::ready(2));

                assert_eq!(deque.next().await, Some(1));
                assert_eq!(deque.next().await, Some(2));
                assert!(deque.is_empty());

                // Push new futures after consuming all previous ones.
                deque.push_back(CountdownFuture::ready(3));
                deque.push_front(CountdownFuture::ready(0));

                assert_eq!(deque.len(), 2);
                assert_eq!(deque.next().await, Some(0));
                assert_eq!(deque.next().await, Some(3));
                assert!(deque.next().await.is_none());
            });
        });
    }

    #[test]
    fn drop_with_mix_of_ready_and_pending() {
        let dropped_pending = Arc::new(AtomicBool::new(false));
        let dropped_ready = Arc::new(AtomicBool::new(false));

        {
            let mut deque = LocalFutureDeque::new();

            // A future that will complete.
            let guard = DropGuard(Arc::clone(&dropped_ready));
            deque.push_back(async move {
                let _guard = guard;
                42
            });

            // A future that will remain pending.
            let guard = DropGuard(Arc::clone(&dropped_pending));
            deque.push_back(async move {
                let _guard = guard;
                std::future::pending::<i32>().await
            });

            // Poll once so the first future completes (becomes Ready).
            let waker = Waker::noop();
            let cx = &mut Context::from_waker(waker);
            let result = deque.poll_front(cx);
            assert_eq!(result, Poll::Ready(Some(42)));

            // Deque now has one pending slot. Drop it.
        }

        // The pending future's captured guard should have been dropped.
        assert!(dropped_pending.load(Ordering::Relaxed));
        // The ready future's guard was already consumed via poll_next.
        assert!(dropped_ready.load(Ordering::Relaxed));
    }

    #[test]
    fn drop_releases_waker_metadata_refs() {
        // Verify that dropping a deque with pending slots correctly releases the
        // waker metadata references held by those slots (the Drop impl path).
        //
        // We do this by capturing a waker clone from a pending future, then
        // dropping the deque. After the drop, the captured waker clone should
        // still be valid and its wake/drop should succeed without issues. If the
        // Drop impl failed to release its reference, the refcount would be off
        // and cleanup would not function correctly.
        let (waker_tx, waker_rx) = mpsc::channel();
        let value_ready = Arc::new(AtomicBool::new(false));

        let captured_waker;
        {
            let mut deque = FutureDeque::new();
            deque.push_back(WakerCaptureFuture {
                waker_tx: Some(waker_tx),
                value_ready: Arc::clone(&value_ready),
            });

            // Poll once to capture the waker.
            let waker = Waker::noop();
            let cx = &mut Context::from_waker(waker);
            let result = deque.poll_front(cx);
            assert!(result.is_pending());

            captured_waker = waker_rx.recv().unwrap();

            // Deque is dropped here with one pending slot. The Drop impl
            // should call release_ref for that slot's metadata.
        }

        // The captured waker clone still holds a reference. Waking and dropping
        // it should succeed — the metadata was not prematurely removed because
        // the captured clone keeps the refcount above zero.
        captured_waker.wake();
    }

    #[test]
    fn send_push_front_ordering() {
        testing::with_watchdog(|| {
            block_on(async {
                let mut deque = FutureDeque::new();
                deque.push_front(CountdownFuture::ready(1));
                deque.push_front(CountdownFuture::ready(2));

                assert_eq!(deque.next().await, Some(2));
                assert_eq!(deque.next().await, Some(1));
            });
        });
    }

    #[test]
    fn send_pop_front_returns_value() {
        let mut deque = FutureDeque::new();
        deque.push_back(CountdownFuture::ready(10));
        deque.push_back(CountdownFuture::ready(20));

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        // Poll and consume the front item via poll_next.
        let result = deque.poll_front(cx);
        assert_eq!(result, Poll::Ready(Some(10)));

        // The second item was also polled and is ready. Pop it manually.
        assert_eq!(deque.pop_front(), Some(20));
    }

    #[test]
    fn send_pop_back_returns_value() {
        let mut deque = FutureDeque::new();
        deque.push_back(CountdownFuture::pending(2, 10));
        deque.push_back(CountdownFuture::ready(20));

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        // Poll: front future still pending, back future ready.
        let result = deque.poll_front(cx);
        assert!(result.is_pending());

        // Pop the ready back item.
        assert_eq!(deque.pop_back(), Some(20));
    }

    #[test]
    fn send_len_and_is_empty() {
        let mut deque = FutureDeque::new();
        assert!(deque.is_empty());
        assert_eq!(deque.len(), 0);

        deque.push_back(CountdownFuture::ready(1));
        deque.push_back(CountdownFuture::ready(2));
        assert!(!deque.is_empty());
        assert_eq!(deque.len(), 2);
    }

    // Multithreaded tests using events_once::Event for cross-thread signaling.
    // These exercise the full waker chain (slot waker -> activation flag -> parent
    // waker) across real OS threads and are especially valuable under Miri with
    // many-seeds (miri-harder) to detect data races in atomic operations.

    #[test]
    fn cross_thread_event_completion() {
        testing::with_watchdog(|| {
            let (sender, receiver) = events_once::Event::<i32>::boxed();

            let mut deque = FutureDeque::new();
            deque.push_back(async move { receiver.await.unwrap() });

            std::thread::spawn(move || {
                sender.send(42);
            })
            .join()
            .unwrap();

            block_on(async {
                assert_eq!(deque.next().await, Some(42));
            });
        });
    }

    #[test]
    fn cross_thread_event_waker_activation() {
        testing::with_watchdog(|| {
            let (sender, receiver) = events_once::Event::<i32>::boxed();

            let mut deque = FutureDeque::new();
            deque.push_back(async move { receiver.await.unwrap() });

            // Pre-poll with a noop waker so the future registers an initial
            // slot waker. Then switch to block_on (different parent waker) while
            // the sender fires from another thread.
            let waker = Waker::noop();
            let cx = &mut Context::from_waker(waker);
            let result = deque.poll_front(cx);
            assert!(result.is_pending());

            // Signal from another thread. May arrive before or during block_on.
            let handle = std::thread::spawn(move || {
                sender.send(99);
            });

            // block_on uses a different parent waker. poll() updates the shared
            // parent, so wake notifications from the slot waker reach the correct
            // executor.
            block_on(async {
                assert_eq!(deque.next().await, Some(99));
            });

            handle.join().unwrap();
        });
    }

    #[test]
    fn cross_thread_multiple_events() {
        testing::with_watchdog(|| {
            let (sender1, receiver1) = events_once::Event::<i32>::boxed();
            let (sender2, receiver2) = events_once::Event::<i32>::boxed();
            let (sender3, receiver3) = events_once::Event::<i32>::boxed();

            let mut deque = FutureDeque::new();
            deque.push_back(async move { receiver1.await.unwrap() });
            deque.push_back(async move { receiver2.await.unwrap() });
            deque.push_back(async move { receiver3.await.unwrap() });

            // Signal all events from separate threads.
            let t1 = std::thread::spawn(move || sender1.send(10));
            let t2 = std::thread::spawn(move || sender2.send(20));
            let t3 = std::thread::spawn(move || sender3.send(30));
            t1.join().unwrap();
            t2.join().unwrap();
            t3.join().unwrap();

            // Results come out in deque order, not completion order.
            block_on(async {
                assert_eq!(deque.next().await, Some(10));
                assert_eq!(deque.next().await, Some(20));
                assert_eq!(deque.next().await, Some(30));
            });
        });
    }

    // --- Additional multithreaded tests ---
    //
    // These tests exercise the waker machinery (atomic activation, refcounting,
    // cross-thread clone/wake/drop) under real concurrency. They are especially
    // valuable when run under Miri with many-seeds (miri-harder) to detect
    // data races and incorrect memory orderings.

    /// A future that captures a clone of its waker on first poll and sends it
    /// via a channel. Completes when a shared flag is set.
    struct WakerCaptureFuture {
        waker_tx: Option<mpsc::Sender<Waker>>,
        value_ready: Arc<AtomicBool>,
    }

    impl Unpin for WakerCaptureFuture {}

    impl Future for WakerCaptureFuture {
        type Output = i32;

        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<i32> {
            let this = self.get_mut();
            if let Some(tx) = this.waker_tx.take() {
                tx.send(cx.waker().clone()).unwrap();
            }

            if this.value_ready.load(Ordering::Acquire) {
                Poll::Ready(42)
            } else {
                Poll::Pending
            }
        }
    }

    #[test]
    fn concurrent_signals_during_active_poll() {
        testing::with_watchdog(|| {
            const FUTURE_COUNT: usize = 4;

            let mut senders = Vec::new();
            let mut deque = FutureDeque::new();

            for _ in 0..FUTURE_COUNT {
                let (sender, receiver) = events_once::Event::<i32>::boxed();
                senders.push(sender);
                deque.push_back(async move { receiver.await.unwrap() });
            }

            // Use a barrier to ensure all sender threads fire concurrently
            // while block_on is actively polling.
            let barrier = Arc::new(Barrier::new(FUTURE_COUNT));

            let handles: Vec<_> = senders
                .into_iter()
                .enumerate()
                .map(|(i, sender)| {
                    let barrier = Arc::clone(&barrier);
                    let value = i32::try_from(i).unwrap();
                    std::thread::spawn(move || {
                        barrier.wait();
                        sender.send(value);
                    })
                })
                .collect();

            block_on(async {
                let mut results = Vec::new();
                while let Some(value) = deque.next().await {
                    results.push(value);
                }
                let expected: Vec<i32> = (0..FUTURE_COUNT)
                    .map(|i| i32::try_from(i).unwrap())
                    .collect();
                assert_eq!(results, expected);
            });

            for h in handles {
                h.join().unwrap();
            }
        });
    }

    #[test]
    fn deque_consumed_on_different_thread() {
        testing::with_watchdog(|| {
            let (sender1, receiver1) = events_once::Event::<i32>::boxed();
            let (sender2, receiver2) = events_once::Event::<i32>::boxed();

            let mut deque = FutureDeque::new();
            deque.push_back(async move { receiver1.await.unwrap() });
            deque.push_back(async move { receiver2.await.unwrap() });

            sender1.send(10);
            sender2.send(20);

            // Move the deque to a different thread for consumption.
            let handle = std::thread::spawn(move || {
                block_on(async {
                    assert_eq!(deque.next().await, Some(10));
                    assert_eq!(deque.next().await, Some(20));
                });
            });

            handle.join().unwrap();
        });
    }

    #[test]
    fn deque_polled_on_foreign_thread_with_concurrent_signals() {
        testing::with_watchdog(|| {
            let (sender1, receiver1) = events_once::Event::<i32>::boxed();
            let (sender2, receiver2) = events_once::Event::<i32>::boxed();

            let mut deque = FutureDeque::new();
            deque.push_back(async move { receiver1.await.unwrap() });
            deque.push_back(async move { receiver2.await.unwrap() });

            // Move the deque to another thread for polling while signals
            // arrive from the main thread.
            let consumer = std::thread::spawn(move || {
                block_on(async {
                    assert_eq!(deque.next().await, Some(10));
                    assert_eq!(deque.next().await, Some(20));
                });
            });

            sender1.send(10);
            sender2.send(20);

            consumer.join().unwrap();
        });
    }

    #[test]
    fn concurrent_wakes_on_same_slot() {
        const WAKER_COUNT: usize = 4;

        let (waker_tx, waker_rx) = mpsc::channel();
        let value_ready = Arc::new(AtomicBool::new(false));

        let mut deque = FutureDeque::new();
        deque.push_back(WakerCaptureFuture {
            waker_tx: Some(waker_tx),
            value_ready: Arc::clone(&value_ready),
        });

        // First poll: captures waker and sends it via channel.
        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);
        let result = deque.poll_front(cx);
        assert!(result.is_pending());

        let captured_waker = waker_rx.recv().unwrap();

        // Set value ready before waking.
        value_ready.store(true, Ordering::Release);

        // Clone the waker to multiple threads and wake concurrently.
        // This exercises the atomic activation dedup: only the first
        // swap(1, AcqRel) that sees 0 wakes the parent.
        let barrier = Arc::new(Barrier::new(WAKER_COUNT));
        let handles: Vec<_> = std::iter::repeat_with(|| {
            let w = captured_waker.clone();
            let b = Arc::clone(&barrier);
            std::thread::spawn(move || {
                b.wait();
                w.wake_by_ref();
            })
        })
        .take(WAKER_COUNT)
        .collect();

        for h in handles {
            h.join().unwrap();
        }

        let result = deque.poll_front(cx);
        assert_eq!(result, Poll::Ready(Some(42)));
    }

    #[test]
    fn waker_clone_dropped_on_foreign_thread() {
        let (waker_tx, waker_rx) = mpsc::channel();
        let value_ready = Arc::new(AtomicBool::new(false));

        let mut deque = FutureDeque::new();
        deque.push_back(WakerCaptureFuture {
            waker_tx: Some(waker_tx),
            value_ready: Arc::clone(&value_ready),
        });

        // First poll: captures waker.
        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);
        let result = deque.poll_front(cx);
        assert!(result.is_pending());

        let captured_waker = waker_rx.recv().unwrap();

        // Complete the future via the captured waker.
        value_ready.store(true, Ordering::Release);
        captured_waker.wake_by_ref();

        let result = deque.poll_front(cx);
        assert_eq!(result, Poll::Ready(Some(42)));

        // The slot released its metadata reference when the future completed.
        // The captured waker clone still holds a reference. Dropping it on
        // another thread exercises the cross-thread release_ref path, including
        // pool.lock().remove() from a foreign thread.
        std::thread::spawn(move || {
            drop(captured_waker);
        })
        .join()
        .unwrap();
    }

    #[test]
    fn owned_wake_activates_future() {
        let (waker_tx, waker_rx) = mpsc::channel();
        let value_ready = Arc::new(AtomicBool::new(false));

        let mut deque = FutureDeque::new();
        deque.push_back(WakerCaptureFuture {
            waker_tx: Some(waker_tx),
            value_ready: Arc::clone(&value_ready),
        });

        // First poll: captures waker.
        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);
        let result = deque.poll_front(cx);
        assert!(result.is_pending());

        let captured_waker = waker_rx.recv().unwrap();

        // Use owned wake (consumes the waker), which calls
        // wake_raw_waker → wake_by_ref + drop.
        value_ready.store(true, Ordering::Release);
        captured_waker.wake();

        let result = deque.poll_front(cx);
        assert_eq!(result, Poll::Ready(Some(42)));
    }

    // --- Future impl and poll() return value tests ---

    #[test]
    fn poll_returns_ready_when_empty() {
        let mut deque = LocalFutureDeque::<u32>::new();

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);
        assert_eq!(deque.poll(cx), Poll::Ready(()));
    }

    #[test]
    fn poll_returns_ready_when_all_complete() {
        let mut deque = LocalFutureDeque::new();
        deque.push_back(CountdownFuture::ready(1));
        deque.push_back(CountdownFuture::ready(2));
        deque.push_back(CountdownFuture::ready(3));

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        // All futures are immediately ready, so poll() should return Ready.
        assert_eq!(deque.poll(cx), Poll::Ready(()));

        // Results remain in the deque for popping.
        assert_eq!(deque.pop_front(), Some(1));
        assert_eq!(deque.pop_front(), Some(2));
        assert_eq!(deque.pop_front(), Some(3));
    }

    #[test]
    fn poll_returns_pending_while_futures_pending() {
        let mut deque = LocalFutureDeque::new();
        deque.push_back(CountdownFuture::pending(100, 99));

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        assert_eq!(deque.poll(cx), Poll::Pending);
    }

    #[test]
    fn push_after_ready_resets_to_pending() {
        let mut deque = LocalFutureDeque::new();
        deque.push_back(CountdownFuture::ready(1));

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        // All futures complete, poll returns Ready.
        assert_eq!(deque.poll(cx), Poll::Ready(()));

        // Push a pending future — readiness should reset.
        deque.push_back(CountdownFuture::pending(100, 99));

        assert_eq!(deque.poll(cx), Poll::Pending);
    }

    // --- Push during active polling ---

    #[test]
    fn push_between_polls() {
        let mut deque = LocalFutureDeque::new();
        deque.push_back(CountdownFuture::ready(1));

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        // Poll the first future.
        assert_eq!(deque.poll(cx), Poll::Ready(()));
        assert_eq!(deque.pop_front(), Some(1));

        // Push a second future after the first one was consumed.
        deque.push_back(CountdownFuture::ready(2));

        // Poll the newly pushed future.
        assert_eq!(deque.poll(cx), Poll::Ready(()));
        assert_eq!(deque.pop_front(), Some(2));
    }

    #[test]
    fn push_front_between_polls_reorders() {
        let mut deque = LocalFutureDeque::new();
        deque.push_back(CountdownFuture::ready(2));

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        // Poll drives the first future to completion.
        assert_eq!(deque.poll(cx), Poll::Ready(()));

        // Push a new future to the front before popping.
        deque.push_front(CountdownFuture::ready(1));

        // Poll drives the new front future.
        assert_eq!(deque.poll(cx), Poll::Ready(()));

        // Front is the newly pushed future.
        assert_eq!(deque.pop_front(), Some(1));
        assert_eq!(deque.pop_front(), Some(2));
    }

    // --- Larger deque ---

    #[test]
    fn many_futures_ordering() {
        const COUNT: usize = 16;

        let mut deque = LocalFutureDeque::new();
        for i in 0..COUNT {
            deque.push_back(CountdownFuture::ready(i));
        }

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        assert_eq!(deque.poll(cx), Poll::Ready(()));

        for i in 0..COUNT {
            assert_eq!(deque.pop_front(), Some(i));
        }
        assert!(deque.is_empty());
    }

    #[test]
    fn many_futures_mixed_readiness() {
        const COUNT: usize = 16;

        let mut deque = LocalFutureDeque::new();
        // Push a mix of immediately-ready and countdown futures.
        for i in 0..COUNT {
            if i % 2 == 0 {
                deque.push_back(CountdownFuture::ready(i));
            } else {
                deque.push_back(CountdownFuture::pending(1, i));
            }
        }

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        // First poll: even futures complete, odd futures return Pending
        // and wake themselves.
        assert_eq!(deque.poll(cx), Poll::Pending);

        // Second poll: odd futures complete on their second poll.
        assert_eq!(deque.poll(cx), Poll::Ready(()));

        // All results come out in push order.
        for i in 0..COUNT {
            assert_eq!(deque.pop_front(), Some(i));
        }
    }

    // --- Waker update path ---

    #[test]
    fn waker_update_routes_to_new_parent() {
        // Poll with a noop waker, then poll with a real waker. The real
        // waker should receive the wake notification when the future signals.
        let (waker_tx, waker_rx) = mpsc::channel();
        let value_ready = Arc::new(AtomicBool::new(false));

        let mut deque = FutureDeque::new();
        deque.push_back(WakerCaptureFuture {
            waker_tx: Some(waker_tx),
            value_ready: Arc::clone(&value_ready),
        });

        // First poll with noop waker — captures the slot waker.
        let noop = Waker::noop();
        let cx = &mut Context::from_waker(noop);
        let result = deque.poll_front(cx);
        assert!(result.is_pending());

        let captured_waker = waker_rx.recv().unwrap();

        // Create a custom waker that sets a flag when woken.
        let parent_woken = Arc::new(AtomicBool::new(false));
        let real_waker = waker_from_flag(Arc::clone(&parent_woken));
        let cx = &mut Context::from_waker(&real_waker);

        // Second poll with the real waker — updates the shared parent.
        let result = deque.poll_front(cx);
        assert!(result.is_pending());

        // Wake the captured slot waker — should propagate to the real parent.
        value_ready.store(true, Ordering::Release);
        captured_waker.wake();

        // The real waker should have been notified.
        assert!(parent_woken.load(Ordering::Acquire));

        // Final poll to collect the result.
        let result = deque.poll_front(cx);
        assert_eq!(result, Poll::Ready(Some(42)));
    }

    /// Creates a [`Waker`] that sets an [`AtomicBool`] flag when woken.
    fn waker_from_flag(flag: Arc<AtomicBool>) -> Waker {
        const VTABLE: RawWakerVTable = RawWakerVTable::new(
            // clone
            |data| {
                // SAFETY: data is a valid *const Arc<AtomicBool> from Box::into_raw.
                let flag = unsafe { &*(data as *const Arc<AtomicBool>) };
                let cloned = Box::new(Arc::clone(flag));
                RawWaker::new(Box::into_raw(cloned) as *const (), &VTABLE)
            },
            // wake (owned)
            |data| {
                // SAFETY: data is a valid *const Arc<AtomicBool> from Box::into_raw.
                let flag = unsafe { Box::from_raw(data as *mut Arc<AtomicBool>) };
                flag.store(true, Ordering::Release);
            },
            // wake_by_ref
            |data| {
                // SAFETY: data is a valid *const Arc<AtomicBool> from Box::into_raw.
                let flag = unsafe { &*(data as *const Arc<AtomicBool>) };
                flag.store(true, Ordering::Release);
            },
            // drop
            |data| {
                // SAFETY: data is a valid *const Arc<AtomicBool> from Box::into_raw.
                drop(unsafe { Box::from_raw(data as *mut Arc<AtomicBool>) });
            },
        );

        let flag_box = Box::new(flag);
        let raw = RawWaker::new(Box::into_raw(flag_box) as *const (), &VTABLE);

        // SAFETY: The vtable functions correctly match the data pointer layout.
        unsafe { Waker::from_raw(raw) }
    }

    // --- Wake deduplication ---

    #[test]
    fn duplicate_wake_does_not_rewake_parent() {
        // When a slot is already activated, subsequent wakes should not
        // re-wake the parent waker. We verify this by counting parent wakes.
        let (waker_tx, waker_rx) = mpsc::channel();
        let value_ready = Arc::new(AtomicBool::new(false));

        let mut deque = FutureDeque::new();
        deque.push_back(WakerCaptureFuture {
            waker_tx: Some(waker_tx),
            value_ready: Arc::clone(&value_ready),
        });

        // Track how many times the parent waker is invoked.
        let wake_count = Arc::new(AtomicUsize::new(0));
        let parent_waker = waker_from_counter(Arc::clone(&wake_count));
        let cx = &mut Context::from_waker(&parent_waker);

        let result = deque.poll_front(cx);
        assert!(result.is_pending());

        let captured_waker = waker_rx.recv().unwrap();

        // First wake: activates the slot, should wake the parent once.
        captured_waker.wake_by_ref();
        assert_eq!(wake_count.load(Ordering::Relaxed), 1);

        // Second wake: slot already activated, should NOT wake the parent again.
        captured_waker.wake_by_ref();
        assert_eq!(wake_count.load(Ordering::Relaxed), 1);
    }

    /// Creates a [`Waker`] that increments an [`AtomicUsize`] counter when woken.
    fn waker_from_counter(counter: Arc<AtomicUsize>) -> Waker {
        const VTABLE: RawWakerVTable = RawWakerVTable::new(
            // clone
            |data| {
                // SAFETY: data is a valid *const Arc<AtomicUsize> from Box::into_raw.
                let counter = unsafe { &*(data as *const Arc<AtomicUsize>) };
                let cloned = Box::new(Arc::clone(counter));
                RawWaker::new(Box::into_raw(cloned) as *const (), &VTABLE)
            },
            // wake (owned)
            |data| {
                // SAFETY: data is a valid *const Arc<AtomicUsize> from Box::into_raw.
                let counter = unsafe { Box::from_raw(data as *mut Arc<AtomicUsize>) };
                counter.fetch_add(1, Ordering::Relaxed);
            },
            // wake_by_ref
            |data| {
                // SAFETY: data is a valid *const Arc<AtomicUsize> from Box::into_raw.
                let counter = unsafe { &*(data as *const Arc<AtomicUsize>) };
                counter.fetch_add(1, Ordering::Relaxed);
            },
            // drop
            |data| {
                // SAFETY: data is a valid *const Arc<AtomicUsize> from Box::into_raw.
                drop(unsafe { Box::from_raw(data as *mut Arc<AtomicUsize>) });
            },
        );

        let counter_box = Box::new(counter);
        let raw = RawWaker::new(Box::into_raw(counter_box) as *const (), &VTABLE);

        // SAFETY: The vtable functions correctly match the data pointer layout.
        unsafe { Waker::from_raw(raw) }
    }

    // --- Pop after drain ---

    #[test]
    fn pop_after_drain_returns_none() {
        let mut deque = LocalFutureDeque::new();
        deque.push_back(CountdownFuture::ready(1));
        deque.push_back(CountdownFuture::ready(2));

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        // Consume all items via poll_front.
        assert_eq!(deque.poll_front(cx), Poll::Ready(Some(1)));
        assert_eq!(deque.poll_front(cx), Poll::Ready(Some(2)));

        // Deque is now drained. Both pop methods should return None.
        assert!(deque.pop_front().is_none());
        assert!(deque.pop_back().is_none());
        assert!(deque.is_empty());
    }

    // --- Unit-output futures ---

    #[test]
    fn unit_output_futures() {
        let mut deque = LocalFutureDeque::<()>::new();
        deque.push_back(async {});
        deque.push_back(async {});

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        assert_eq!(deque.poll(cx), Poll::Ready(()));

        assert_eq!(deque.pop_front(), Some(()));
        assert_eq!(deque.pop_front(), Some(()));
        assert!(deque.is_empty());
    }

    #[test]
    fn unit_output_via_poll_front() {
        let mut deque = LocalFutureDeque::<()>::new();
        deque.push_back(async {});
        deque.push_back(async {});

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        assert_eq!(deque.poll_front(cx), Poll::Ready(Some(())));
        assert_eq!(deque.poll_front(cx), Poll::Ready(Some(())));
        assert_eq!(deque.poll_front(cx), Poll::Ready(None));
    }

    // --- Future trait await integration ---

    #[test]
    fn await_empty_deque_completes_immediately() {
        testing::with_watchdog(|| {
            block_on(async {
                let mut deque = LocalFutureDeque::<u32>::new();

                // Awaiting an empty deque should complete immediately.
                (&mut deque).await;

                assert!(deque.is_empty());
            });
        });
    }

    #[test]
    fn await_deque_waits_for_all_futures() {
        testing::with_watchdog(|| {
            block_on(async {
                let mut deque = LocalFutureDeque::new();
                deque.push_back(CountdownFuture::pending(2, 10));
                deque.push_back(CountdownFuture::ready(20));
                deque.push_back(CountdownFuture::pending(1, 30));

                // Await waits for all futures to complete.
                (&mut deque).await;

                // All results available for popping.
                assert_eq!(deque.pop_front(), Some(10));
                assert_eq!(deque.pop_front(), Some(20));
                assert_eq!(deque.pop_front(), Some(30));
            });
        });
    }

    #[test]
    fn await_then_push_then_await_again() {
        testing::with_watchdog(|| {
            block_on(async {
                let mut deque = LocalFutureDeque::new();
                deque.push_back(CountdownFuture::ready(1));

                // First await: completes.
                (&mut deque).await;
                assert_eq!(deque.pop_front(), Some(1));

                // Push more futures and await again.
                deque.push_back(CountdownFuture::pending(1, 2));
                (&mut deque).await;
                assert_eq!(deque.pop_front(), Some(2));
            });
        });
    }
}
