use std::{
    collections::VecDeque,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Waker},
};

use std::sync::Mutex;

use crate::{
    erased_future::ErasedFuture,
    waker_meta::{self, MetaPtr},
};

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
    fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    // Callers always check is_ready() before calling take_value(), making the
    // Pending branch unreachable under normal operation. However, returning Option
    // lets callers use combinators like and_then, and also makes the code safe against
    // future refactors that might remove the precondition check.
    #[cfg_attr(coverage_nightly, coverage(off))]
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
    pub(crate) fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Pops the front result if the frontmost future has completed.
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
    #[cfg_attr(test, mutants::skip)]
    #[cfg_attr(coverage_nightly, coverage(off))] // Only runs when deque is dropped with
    // pending slots, which is a cleanup path.
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
    use std::{
        future::Future,
        pin::Pin,
        sync::{
            Arc, Barrier,
            atomic::{AtomicBool, AtomicUsize, Ordering},
            mpsc,
        },
        task::{Context, Poll, Waker},
    };

    use futures::{StreamExt, executor::block_on};

    use crate::{FutureDeque, LocalFutureDeque};

    /// Starts a background thread (at most once) that terminates the process
    /// after 10 seconds. Prevents tests from hanging indefinitely on mutations
    /// that break the poll loop.
    #[allow(clippy::exit, reason = "watchdog must terminate the process on hang")]
    fn watchdog() {
        // The watchdog is only needed for mutation testing, which runs under
        // native cargo test. Under Miri, tests are too slow for a timed watchdog.
        #[cfg(not(miri))]
        {
            use std::{sync::Once, time::Duration};

            static INIT: Once = Once::new();
            INIT.call_once(|| {
                std::thread::spawn(|| {
                    std::thread::sleep(Duration::from_secs(10));
                    std::process::exit(1);
                });
            });
        }
    }

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
        watchdog();

        block_on(async {
            let mut deque = LocalFutureDeque::<u32>::new();
            assert!(deque.next().await.is_none());
        });
    }

    #[test]
    fn push_back_pop_front_single() {
        watchdog();

        block_on(async {
            let mut deque = LocalFutureDeque::new();
            deque.push_back(CountdownFuture::ready(42));

            assert_eq!(deque.len(), 1);
            let value = deque.next().await;
            assert_eq!(value, Some(42));
            assert!(deque.is_empty());
        });
    }

    #[test]
    fn push_back_pop_front_ordering() {
        watchdog();

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
    }

    #[test]
    fn push_front_pop_front_ordering() {
        watchdog();

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
        watchdog();

        block_on(async {
            let mut deque = LocalFutureDeque::new();
            deque.push_back(CountdownFuture::ready(2));
            deque.push_front(CountdownFuture::ready(1));
            deque.push_back(CountdownFuture::ready(3));

            assert_eq!(deque.next().await, Some(1));
            assert_eq!(deque.next().await, Some(2));
            assert_eq!(deque.next().await, Some(3));
        });
    }

    #[test]
    fn pending_futures_eventually_complete() {
        watchdog();

        block_on(async {
            let mut deque = LocalFutureDeque::new();
            deque.push_back(CountdownFuture::pending(3, 100));
            deque.push_back(CountdownFuture::pending(1, 200));

            // The stream will poll futures until the front one completes.
            assert_eq!(deque.next().await, Some(100));
            assert_eq!(deque.next().await, Some(200));
        });
    }

    #[test]
    fn front_blocks_stream_even_if_back_is_ready() {
        watchdog();

        block_on(async {
            let mut deque = LocalFutureDeque::new();
            deque.push_back(CountdownFuture::pending(2, 10));
            deque.push_back(CountdownFuture::ready(20));

            // Stream yields front first, even though back is ready sooner.
            assert_eq!(deque.next().await, Some(10));
            assert_eq!(deque.next().await, Some(20));
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
        watchdog();

        block_on(async {
            let mut deque = FutureDeque::new();
            deque.push_back(CountdownFuture::ready(42));
            assert_eq!(deque.next().await, Some(42));
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
        watchdog();

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
    fn send_push_front_ordering() {
        watchdog();

        block_on(async {
            let mut deque = FutureDeque::new();
            deque.push_front(CountdownFuture::ready(1));
            deque.push_front(CountdownFuture::ready(2));

            assert_eq!(deque.next().await, Some(2));
            assert_eq!(deque.next().await, Some(1));
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
        watchdog();

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
    }

    #[test]
    fn cross_thread_event_waker_activation() {
        watchdog();

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
    }

    #[test]
    fn cross_thread_multiple_events() {
        watchdog();

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
        const FUTURE_COUNT: usize = 4;

        watchdog();

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
    }

    #[test]
    fn deque_consumed_on_different_thread() {
        watchdog();

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
    }

    #[test]
    fn deque_polled_on_foreign_thread_with_concurrent_signals() {
        watchdog();

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
    }

    #[test]
    fn concurrent_wakes_on_same_slot() {
        const WAKER_COUNT: usize = 4;

        watchdog();

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
        watchdog();

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
}
