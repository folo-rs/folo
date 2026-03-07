use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Wake, Waker},
};

use infinity_pool::{RawBlindPool, RawBlindPooledMut};

use crate::deque_future::{DequeFuture, RawPooledCastDequeFuture as _};

/// Shared core implementation for both [`FutureDeque`][crate::FutureDeque]
/// and [`LocalFutureDeque`][crate::LocalFutureDeque].
pub(crate) struct FutureDequeCore<T> {
    pool: RawBlindPool,
    slots: VecDeque<Slot<T>>,
}

enum Slot<T> {
    Pending {
        handle: RawBlindPooledMut<dyn DequeFuture<T>>,
        activated: Arc<AtomicBool>,
        slot_waker: Option<Arc<SlotWaker>>,
    },
    Ready {
        value: T,
    },
}

impl<T> Slot<T> {
    fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    fn take_value(self) -> Option<T> {
        match self {
            Self::Ready { value } => Some(value),
            Self::Pending { .. } => None,
        }
    }
}

/// Custom waker that sets a per-slot activation flag and wakes the parent task.
struct SlotWaker {
    activated: Arc<AtomicBool>,
    parent_waker: Waker,
}

impl Wake for SlotWaker {
    // Rarely called; most futures use wake_by_ref to avoid consuming the waker.
    // The companion wake_by_ref is exercised by tests.
    #[cfg_attr(test, mutants::skip)]
    fn wake(self: Arc<Self>) {
        self.activated.store(true, Ordering::Release);
        self.parent_waker.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.activated.store(true, Ordering::Release);
        self.parent_waker.wake_by_ref();
    }
}

impl<T> FutureDequeCore<T> {
    pub(crate) fn new() -> Self {
        Self {
            pool: RawBlindPool::new(),
            slots: VecDeque::new(),
        }
    }

    /// Returns the number of entries (both pending and ready) in the deque.
    pub(crate) fn len(&self) -> usize {
        self.slots.len()
    }

    /// Returns `true` if the deque contains no entries.
    pub(crate) fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Adds a future to the back of the deque.
    pub(crate) fn push_back<F: DequeFuture<T> + 'static>(&mut self, future: F) {
        let handle = self.pool.insert(future);

        // SAFETY: The concrete type F implements DequeFuture<T>, so the cast is valid.
        let handle = unsafe { handle.cast_deque_future::<T>() };

        self.slots.push_back(Slot::Pending {
            handle,
            activated: Arc::new(AtomicBool::new(true)),
            slot_waker: None,
        });
    }

    /// Adds a future to the front of the deque.
    pub(crate) fn push_front<F: DequeFuture<T> + 'static>(&mut self, future: F) {
        let handle = self.pool.insert(future);

        // SAFETY: The concrete type F implements DequeFuture<T>, so the cast is valid.
        let handle = unsafe { handle.cast_deque_future::<T>() };

        self.slots.push_front(Slot::Pending {
            handle,
            activated: Arc::new(AtomicBool::new(true)),
            slot_waker: None,
        });
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

    /// Polls all activated futures front-to-back, transitioning completed ones to ready.
    pub(crate) fn drive(&mut self, cx: &Context<'_>) {
        Self::drive_inner(&mut self.pool, &mut self.slots, cx);
    }

    fn drive_inner(pool: &mut RawBlindPool, slots: &mut VecDeque<Slot<T>>, cx: &Context<'_>) {
        for slot in slots.iter_mut() {
            let poll_result = {
                let Slot::Pending {
                    handle,
                    activated,
                    slot_waker,
                } = slot
                else {
                    continue;
                };

                // If the parent waker has changed since the last poll, the future
                // must be re-polled to receive an updated SlotWaker — otherwise
                // wake notifications would be routed to the stale parent.
                let parent_changed = slot_waker
                    .as_ref()
                    .is_some_and(|sw| !sw.parent_waker.will_wake(cx.waker()));

                // Atomically read and clear the activation flag. Using swap ensures
                // that a concurrent wake between the read and clear is not lost.
                // Re-poll if activated OR if the parent waker has changed.
                if !activated.swap(false, Ordering::AcqRel) && !parent_changed {
                    continue;
                }

                // Create or reuse the SlotWaker.
                if parent_changed || slot_waker.is_none() {
                    *slot_waker = Some(Arc::new(SlotWaker {
                        activated: Arc::clone(activated),
                        parent_waker: cx.waker().clone(),
                    }));
                }

                let waker = Waker::from(Arc::clone(
                    slot_waker
                        .as_ref()
                        .expect("we always populate the slot waker above"),
                ));
                let sub_cx = &mut Context::from_waker(&waker);

                // SAFETY: The pool guarantees that the handle points to a valid,
                // pinned allocation that has not been removed.
                let pin_fut = unsafe { handle.as_pin_mut() };
                pin_fut.poll_deque(sub_cx)
            };

            if let Poll::Ready(value) = poll_result {
                let old = std::mem::replace(slot, Slot::Ready { value });
                if let Slot::Pending { handle, .. } = old {
                    // SAFETY: We own the handle and the pool; the handle came from
                    // this pool's insert call and has not been removed yet.
                    unsafe {
                        pool.remove(handle);
                    }
                }
            }
        }
    }

    /// Drives all futures and pops the front if ready. Used by `Stream::poll_next`.
    pub(crate) fn poll_next(&mut self, cx: &Context<'_>) -> Poll<Option<T>> {
        self.drive(cx);

        if let Some(value) = self.pop_front() {
            Poll::Ready(Some(value))
        } else if self.is_empty() {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }
}

impl<T> Drop for FutureDequeCore<T> {
    // Defense in depth: ensure no pending futures remain in the pool before it is dropped.
    #[cfg_attr(test, mutants::skip)]
    fn drop(&mut self) {
        // We must remove all pending futures from the pool before the pool is dropped,
        // because RawBlindPool frees its backing memory but does not invoke the drop
        // logic of stored values on its own.
        for slot in self.slots.drain(..) {
            if let Slot::Pending { handle, .. } = slot {
                // SAFETY: Each handle was inserted into this pool and has not been removed.
                unsafe {
                    self.pool.remove(handle);
                }
            }
        }
    }
}

// The futures stored in the deque are pinned by the RawBlindPool's heap-allocated slabs,
// not by FutureDequeCore's own fields. The FutureDequeCore struct only holds handles
// (pointers + metadata) and result values, none of which require pinning guarantees.
impl<T> Unpin for FutureDequeCore<T> {}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        pin::Pin,
        sync::{
            Arc, Once,
            atomic::{AtomicBool, Ordering},
        },
        task::{Context, Poll, Waker},
        time::Duration,
    };

    use futures::{Stream, StreamExt, executor::block_on};

    use crate::{FutureDeque, LocalFutureDeque};

    /// Starts a background thread (at most once) that terminates the process
    /// after 10 seconds. Prevents tests from hanging indefinitely on mutations
    /// that break the drive or poll loop.
    #[allow(clippy::exit, reason = "watchdog must terminate the process on hang")]
    fn watchdog() {
        // The watchdog is only needed for mutation testing, which runs under
        // native cargo test. Under Miri, tests are too slow for a timed watchdog.
        #[cfg(not(miri))]
        {
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

        // Drive all futures by calling poll_next via the stream.
        // The front is not ready yet, so stream returns Pending (internally).
        // But pop_back should return the completed back future.
        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);
        let poll = Pin::new(&mut deque).poll_next(cx);
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
        let poll = Pin::new(&mut deque).poll_next(cx);
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

            // The stream will drive futures until the front one completes.
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
        let result = Pin::new(&mut deque).poll_next(cx);
        assert_eq!(result, Poll::Ready(Some(42)));
    }

    #[test]
    fn poll_next_returns_none_for_empty_deque() {
        let mut deque = LocalFutureDeque::<u32>::new();

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);
        let result = Pin::new(&mut deque).poll_next(cx);
        assert_eq!(result, Poll::Ready(None));
    }

    #[test]
    fn multi_poll_future_completes_via_activation() {
        let mut deque = LocalFutureDeque::new();
        deque.push_back(CountdownFuture::pending(1, 42));

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        // First poll: future returns Pending and sets activation via SlotWaker.
        let result = Pin::new(&mut deque).poll_next(cx);
        assert!(result.is_pending());

        // Second poll: future is re-activated and completes.
        let result = Pin::new(&mut deque).poll_next(cx);
        assert_eq!(result, Poll::Ready(Some(42)));
    }

    #[test]
    fn local_pop_front_returns_value_after_drive() {
        let mut deque = LocalFutureDeque::new();
        deque.push_back(CountdownFuture::ready(10));
        deque.push_back(CountdownFuture::ready(20));

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        // Drive and consume the front item via poll_next.
        let result = Pin::new(&mut deque).poll_next(cx);
        assert_eq!(result, Poll::Ready(Some(10)));

        // The second item was also driven and is ready. Pop it manually.
        assert_eq!(deque.pop_front(), Some(20));
    }

    #[test]
    fn local_is_empty_returns_false_when_non_empty() {
        let mut deque = LocalFutureDeque::new();
        deque.push_back(CountdownFuture::ready(42));
        assert!(!deque.is_empty());
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

        // Drive and consume the front item via poll_next.
        let result = Pin::new(&mut deque).poll_next(cx);
        assert_eq!(result, Poll::Ready(Some(10)));

        // The second item was also driven and is ready. Pop it manually.
        assert_eq!(deque.pop_front(), Some(20));
    }

    #[test]
    fn send_pop_back_returns_value() {
        let mut deque = FutureDeque::new();
        deque.push_back(CountdownFuture::pending(2, 10));
        deque.push_back(CountdownFuture::ready(20));

        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);

        // Drive: front future still pending, back future ready.
        let result = Pin::new(&mut deque).poll_next(cx);
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
    // These exercise the full waker chain (SlotWaker -> activation flag -> parent
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
        // SlotWaker. Then switch to block_on (different parent waker) while
        // the sender fires from another thread.
        let waker = Waker::noop();
        let cx = &mut Context::from_waker(waker);
        let result = Pin::new(&mut deque).poll_next(cx);
        assert!(result.is_pending());

        // Signal from another thread. May arrive before or during block_on.
        let handle = std::thread::spawn(move || {
            sender.send(99);
        });

        // block_on uses a different parent waker. drive_inner detects the
        // change and re-polls the future so it receives an updated SlotWaker.
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
}
