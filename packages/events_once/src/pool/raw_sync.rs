use std::any::type_name;
#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomData;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::pin::Pin;
use std::ptr::NonNull;
#[cfg(debug_assertions)]
use std::sync::Arc;
use std::sync::Mutex;

use crate::{
    NEVER_POISONED, PoolState, RawPooledReceiver, RawPooledRef, RawPooledSender, ReceiverCore,
    SenderCore,
};

/// A pool of reusable thread-safe one-time events with manual pool lifecycle management.
///
/// # Examples
///
/// ```
/// use events_once::RawEventPool;
///
/// # #[tokio::main]
/// # async fn main() {
/// let pool = Box::pin(RawEventPool::<String>::new());
///
/// for i in 0..3 {
///     // SAFETY: We promise the pool outlives both the returned endpoints.
///     let (tx, rx) = unsafe { pool.as_ref().rent() };
///
///     tx.send(format!("Message {i}"));
///
///     let message = rx.await.unwrap();
///     println!("{message}");
/// }
/// # }
/// ```
pub struct RawEventPool<T: 'static> {
    // This is in an UnsafeCell to logically "detach" it from the parent object.
    // We will create direct (shared) references to the contents of the cell not only from
    // the pool but also from the event references themselves. This is safe as long as
    // we never create conflicting references. We could not guarantee that for the parent
    // object but we can guarantee it for the cell contents.
    core: NonNull<UnsafeCell<RawEventPoolCore<T>>>,

    // The pointer conveys no ownership, so this marker is what records that the pool owns the
    // values of `T` stored in the events it hands out. The managed pools need no equivalent
    // because their `Arc`/`Rc` core field already expresses that ownership.
    _owns_some: PhantomData<T>,
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: Send + 'static> fmt::Debug for RawEventPool<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("core", &self.core)
            .finish()
    }
}

impl<T: 'static> Drop for RawEventPool<T> {
    fn drop(&mut self) {
        // SAFETY: `self.core` is the unchanged pointer that the matching `Box::into_raw()` in
        // `new()` produced, so it carries the provenance and layout of that same allocation, and
        // no other code path replaces the field or rebuilds an owning box - this is the only
        // conversion back into a `Box`, so the allocation is freed exactly once. Every caller of
        // `rent()` promised that the pool outlives the endpoints it handed out, so no endpoint
        // can reach the core by the time the pool is dropped, and dropping the pool itself
        // requires exclusive access to it.
        drop(unsafe { Box::from_raw(self.core.as_ptr()) });
    }
}

pub(crate) struct RawEventPoolCore<T: 'static> {
    pub(crate) state: Mutex<PoolState<T>>,
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: Send + 'static> fmt::Debug for RawEventPoolCore<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("state", &self.state)
            .finish()
    }
}

impl<T: Send + 'static> RawEventPool<T> {
    /// Creates a new empty event pool.
    #[must_use]
    pub fn new() -> Self {
        let core = RawEventPoolCore {
            state: Mutex::new(PoolState::new()),
        };

        let core_ptr = Box::into_raw(Box::new(UnsafeCell::new(core)));

        Self {
            // SAFETY: Boxed object is never null.
            core: unsafe { NonNull::new_unchecked(core_ptr) },
            _owns_some: PhantomData,
        }
    }

    /// Returns a shared reference to the core.
    fn core(&self) -> &RawEventPoolCore<T> {
        // SAFETY: The pointer comes from the `Box::into_raw()` in `new()` and is never replaced,
        // so it names a live, initialized, correctly aligned core; the allocation is freed only
        // by `Drop`, which needs exclusive access to the pool and therefore cannot run while
        // this shared borrow exists.
        let core_cell = unsafe { self.core.as_ref() };

        // SAFETY: The core is reached only through this method and through the equivalent
        // accessor on the event references, both of which produce shared references, so no
        // exclusive reference to the core can alias this one. Mutation of the core happens
        // exclusively behind its mutex.
        unsafe { &*core_cell.get() }
    }

    /// Rents an event from the pool, returning its endpoints.
    ///
    /// The event will be returned to the pool when both endpoints are dropped.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the pool outlives the endpoints.
    #[must_use]
    pub unsafe fn rent(self: Pin<&Self>) -> (RawPooledSender<T>, RawPooledReceiver<T>) {
        let event = self.core().state.lock().expect(NEVER_POISONED).rent();

        // SAFETY: The event was just rented from this pool's state and has not been released.
        // The endpoints below and the pool's debug-only registry are the only reachers of the
        // event, and none of them creates an exclusive reference to it. Our own caller promised
        // that this pool - the owner of the core - outlives both endpoints.
        let event_ref = unsafe {
            RawPooledRef::new(
                #[cfg(debug_assertions)]
                self.core,
                event,
            )
        };

        let inner_sender = SenderCore::new(event_ref.clone());
        let inner_receiver = ReceiverCore::new(event_ref);

        (
            RawPooledSender::new(inner_sender),
            RawPooledReceiver::new(inner_receiver),
        )
    }

    /// Returns `true` if no events have currently been rented from the pool.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.core().state.lock().expect(NEVER_POISONED).is_empty()
    }

    /// Returns the number of events that have currently been rented from the pool.
    #[must_use]
    pub fn len(&self) -> usize {
        self.core().state.lock().expect(NEVER_POISONED).len()
    }

    /// Uses the provided closure to inspect the backtraces of the most recent awaiter of each
    /// awaited event in the pool.
    ///
    /// This method is only available in debug builds (`cfg(debug_assertions)`).
    /// For any data to be present, `RUST_BACKTRACE=1` or `RUST_LIB_BACKTRACE=1` must be set.
    ///
    /// The closure is called once for each event in the pool that has been awaited at some point
    /// in the past.
    ///
    /// # Reentrancy
    ///
    /// The closure may freely use this pool: it may rent events, drop endpoints of events it
    /// obtained earlier and call [`inspect_awaiters()`][Self::inspect_awaiters] again. The
    /// backtraces are snapshotted before the first call to the closure, so the sequence of
    /// backtraces the closure receives is unaffected by what the closure does to the pool.
    #[cfg(debug_assertions)]
    pub fn inspect_awaiters(&self, mut f: impl FnMut(&Backtrace)) {
        for backtrace in self.awaiter_backtraces() {
            f(&backtrace);
        }
    }

    /// Snapshots the backtrace of the most recent awaiter of each awaited event in the pool.
    ///
    /// The pool lock is released before this returns, so the caller may pass the snapshots to
    /// user-supplied code without holding any lock. Each snapshot is a shared owner of the
    /// backtrace, so it stays valid even if its event is released in the meantime.
    #[cfg(debug_assertions)]
    pub(crate) fn awaiter_backtraces(&self) -> Vec<Arc<Backtrace>> {
        self.core()
            .state
            .lock()
            .expect(NEVER_POISONED)
            .awaiter_backtraces()
    }
}

impl<T: Send + 'static> Default for RawEventPool<T> {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: Automatic inference is unavailable only because of the `NonNull` field. Moving the
// pool moves that pointer while the core allocation stays where it is, and the pool reaches the
// core state exclusively through `RawEventPoolCore::state`, whose mutex synchronizes every such
// access. Values of `T` stored in rented events become reachable from whichever thread the pool
// moved to, so the payload must itself be movable between threads.
// The `'static` bound is already on the struct, so it is not repeated here. Repeating it
// would trigger a rustc bug (rust-lang/rust#110338) in async generator Send inference
// with trait object type params.
unsafe impl<T: Send> Send for RawEventPool<T> {}

// SAFETY: Automatic inference is unavailable only because of the `NonNull` field. A shared
// reference to the pool grants renting and diagnostics, all of which reach `PoolState` through
// the core mutex; the debug-only registry that endpoints touch when releasing an event sits
// behind the same mutex. Concurrent shared use therefore never produces unsynchronized access to
// the core. Renting from several threads places values of `T` into events that other threads
// observe, so the payload must be movable between threads.
unsafe impl<T: Send> Sync for RawEventPool<T> {}

// The NonNull<UnsafeCell<RawEventPoolCore<T>>> field disables auto-trait inference for
// UnwindSafe/RefUnwindSafe. The pool state is mutated only while its mutex is held and no such
// mutation can unwind, so a pool observed after a panic still has consistent slot bookkeeping.
// This holds regardless of the payload, which the pool never exposes: a value is reachable only
// through the endpoints of the event that carries it.
impl<T: Send + 'static> UnwindSafe for RawEventPool<T> {}
impl<T: Send + 'static> RefUnwindSafe for RawEventPool<T> {}

#[cfg(test)]
#[allow(clippy::undocumented_unsafe_blocks, reason = "test code, be concise")]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #[cfg(debug_assertions)]
    use std::cell::RefCell;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::sync::{Arc, Barrier};
    use std::task::{self, Poll, Waker};
    use std::{iter, thread};

    use futures::executor::block_on;
    use static_assertions::assert_impl_all;
    #[cfg(debug_assertions)]
    use testing::assert_panics_with;
    use testing::with_watchdog;

    use super::*;
    use crate::Disconnected;
    #[cfg(debug_assertions)]
    use crate::assert_inspect_awaiters_is_reentrant;

    // The payload satisfies only the bound that the pool's API requires (`Send`) and lacks every
    // trait asserted here, so each of them is supplied by the pool's own synchronization and
    // storage rather than inherited from the payload. A trait object payload also has to preserve
    // the thread-safety traits (regression test for #142).
    assert_impl_all!(RawEventPool<Box<dyn Send>>: Send, Sync, UnwindSafe, RefUnwindSafe);

    #[test]
    fn len() {
        let pool = Box::pin(RawEventPool::<i32>::new());

        assert_eq!(pool.len(), 0);

        let (sender1, receiver1) = unsafe { pool.as_ref().rent() };
        assert_eq!(pool.len(), 1);

        let (sender2, receiver2) = unsafe { pool.as_ref().rent() };
        assert_eq!(pool.len(), 2);

        drop(sender1);
        drop(receiver1);
        assert_eq!(pool.len(), 1);

        drop(sender2);
        drop(receiver2);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn send_receive() {
        let pool = Box::pin(RawEventPool::<i32>::new());

        assert!(pool.is_empty());

        let (sender, receiver) = unsafe { pool.as_ref().rent() };

        assert!(!pool.is_empty());

        {
            let mut receiver = Box::pin(receiver);

            sender.send(42);

            let mut cx = task::Context::from_waker(Waker::noop());

            let poll_result = receiver.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Ready(Ok(42))));
        }

        assert!(pool.is_empty());
    }

    #[test]
    fn send_receive_reused() {
        const ITERATIONS: usize = 32;

        let pool = Box::pin(RawEventPool::<i32>::new());

        assert!(pool.is_empty());

        for _ in 0..ITERATIONS {
            let (sender, receiver) = unsafe { pool.as_ref().rent() };
            let mut receiver = Box::pin(receiver);

            sender.send(42);

            let mut cx = task::Context::from_waker(Waker::noop());

            let poll_result = receiver.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Ready(Ok(42))));
        }

        assert!(pool.is_empty());
    }

    #[test]
    fn send_receive_reused_batches() {
        const ITERATIONS: usize = 4;
        const BATCH_SIZE: usize = 8;

        let pool = Box::pin(RawEventPool::<i32>::new());

        for _ in 0..ITERATIONS {
            let endpoints = iter::repeat_with(|| unsafe { pool.as_ref().rent() })
                .take(BATCH_SIZE)
                .collect::<Vec<_>>();

            for (sender, receiver) in endpoints {
                let mut receiver = Box::pin(receiver);

                sender.send(42);

                let mut cx = task::Context::from_waker(Waker::noop());

                let poll_result = receiver.as_mut().poll(&mut cx);
                assert!(matches!(poll_result, Poll::Ready(Ok(42))));
            }
        }
    }

    #[test]
    fn drop_send() {
        let pool = Box::pin(RawEventPool::<i32>::new());

        let (sender, _) = unsafe { pool.as_ref().rent() };

        sender.send(42);
    }

    #[test]
    fn drop_receive() {
        let pool = Box::pin(RawEventPool::<i32>::new());

        let (_, receiver) = unsafe { pool.as_ref().rent() };
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn receive_drop_receive() {
        let pool = Box::pin(RawEventPool::<i32>::new());

        let (sender, receiver) = unsafe { pool.as_ref().rent() };
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(sender);

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn receive_drop_send() {
        let pool = Box::pin(RawEventPool::<i32>::new());

        let (sender, receiver) = unsafe { pool.as_ref().rent() };
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(receiver);

        sender.send(42);
    }

    #[test]
    fn receive_drop_drop_receiver_first() {
        let pool = Box::pin(RawEventPool::<i32>::new());

        let (sender, receiver) = unsafe { pool.as_ref().rent() };
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(receiver);
        drop(sender);
    }

    #[test]
    fn receive_drop_drop_sender_first() {
        let pool = Box::pin(RawEventPool::<i32>::new());

        let (sender, receiver) = unsafe { pool.as_ref().rent() };
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(sender);
        drop(receiver);
    }

    #[test]
    fn drop_drop_receiver_first() {
        let pool = Box::pin(RawEventPool::<i32>::new());

        let (sender, receiver) = unsafe { pool.as_ref().rent() };

        drop(receiver);
        drop(sender);
    }

    #[test]
    fn drop_drop_sender_first() {
        let pool = Box::pin(RawEventPool::<i32>::new());

        let (sender, receiver) = unsafe { pool.as_ref().rent() };

        drop(sender);
        drop(receiver);
    }

    #[test]
    fn is_ready() {
        let pool = Box::pin(RawEventPool::<i32>::new());

        let (sender, receiver) = unsafe { pool.as_ref().rent() };
        let mut receiver = Box::pin(receiver);

        assert!(!receiver.is_ready());

        sender.send(42);

        assert!(receiver.is_ready());

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn drop_is_ready() {
        let pool = Box::pin(RawEventPool::<i32>::new());

        let (sender, receiver) = unsafe { pool.as_ref().rent() };
        let mut receiver = Box::pin(receiver);

        assert!(!receiver.is_ready());

        drop(sender);

        assert!(receiver.is_ready());

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn into_value() {
        let pool = Box::pin(RawEventPool::<i32>::new());

        let (sender, receiver) = unsafe { pool.as_ref().rent() };

        let Err(crate::IntoValueError::Pending(receiver)) = receiver.into_value() else {
            panic!("Expected receiver to not be ready");
        };

        sender.send(42);

        assert!(matches!(receiver.into_value(), Ok(42)));
    }

    #[test]
    #[should_panic]
    fn panic_poll_after_completion() {
        let pool = Box::pin(RawEventPool::<i32>::new());

        let (sender, receiver) = unsafe { pool.as_ref().rent() };
        let mut receiver = Box::pin(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        assert!(matches!(
            receiver.as_mut().poll(&mut cx),
            Poll::Ready(Ok(42))
        ));

        _ = receiver.as_mut().poll(&mut cx);
    }

    #[test]
    #[should_panic]
    fn panic_is_ready_after_completion() {
        let pool = Box::pin(RawEventPool::<i32>::new());

        let (sender, receiver) = unsafe { pool.as_ref().rent() };
        let mut receiver = Box::pin(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        assert!(matches!(
            receiver.as_mut().poll(&mut cx),
            Poll::Ready(Ok(42))
        ));

        _ = receiver.is_ready();
    }

    #[test]
    fn send_receive_mt() {
        with_watchdog(|| {
            let pool = Box::pin(RawEventPool::<i32>::new());

            let (sender, receiver) = unsafe { pool.as_ref().rent() };

            thread::spawn(move || {
                sender.send(42);
            })
            .join()
            .unwrap();

            thread::spawn(move || {
                let mut receiver = Box::pin(receiver);
                let mut cx = task::Context::from_waker(Waker::noop());

                let poll_result = receiver.as_mut().poll(&mut cx);
                assert!(matches!(poll_result, Poll::Ready(Ok(42))));
            })
            .join()
            .unwrap();
        });
    }

    #[test]
    fn receive_send_receive_mt() {
        with_watchdog(|| {
            let pool = Box::pin(RawEventPool::<i32>::new());

            let (sender, receiver) = unsafe { pool.as_ref().rent() };

            let first_poll_completed = Arc::new(Barrier::new(2));
            let first_poll_completed_clone = Arc::clone(&first_poll_completed);

            let send_thread = thread::spawn(move || {
                first_poll_completed.wait();

                sender.send(42);
            });

            let receive_thread = thread::spawn(move || {
                let mut receiver = Box::pin(receiver);
                let mut cx = task::Context::from_waker(Waker::noop());

                let poll_result = receiver.as_mut().poll(&mut cx);
                assert!(matches!(poll_result, Poll::Pending));

                first_poll_completed_clone.wait();

                // We do not know how many polls this will take, so we switch into real async.
                block_on(async {
                    let result = &mut receiver.await;
                    assert!(matches!(result, Ok(42)));
                });
            });

            send_thread.join().unwrap();
            receive_thread.join().unwrap();
        });
    }

    #[test]
    fn send_receive_unbiased_mt() {
        with_watchdog(|| {
            let pool = Box::pin(RawEventPool::<i32>::new());

            let (sender, receiver) = unsafe { pool.as_ref().rent() };

            let receive_thread = thread::spawn(move || {
                block_on(async {
                    let result = &mut receiver.await;
                    assert!(matches!(result, Ok(42)));
                });
            });

            let send_thread = thread::spawn(move || {
                sender.send(42);
            });

            send_thread.join().unwrap();
            receive_thread.join().unwrap();
        });
    }

    #[test]
    fn drop_receive_unbiased_mt() {
        with_watchdog(|| {
            let pool = Box::pin(RawEventPool::<i32>::new());

            let (sender, receiver) = unsafe { pool.as_ref().rent() };

            let receive_thread = thread::spawn(move || {
                block_on(async {
                    let result = &mut receiver.await;
                    assert!(matches!(result, Err(Disconnected)));
                });
            });

            let send_thread = thread::spawn(move || {
                drop(sender);
            });

            send_thread.join().unwrap();
            receive_thread.join().unwrap();
        });
    }

    #[test]
    fn drop_send_unbiased_mt() {
        with_watchdog(|| {
            let pool = Box::pin(RawEventPool::<i32>::new());

            let (sender, receiver) = unsafe { pool.as_ref().rent() };

            let receive_thread = thread::spawn(move || {
                drop(receiver);
            });

            let send_thread = thread::spawn(move || {
                sender.send(42);
            });

            send_thread.join().unwrap();
            receive_thread.join().unwrap();
        });
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiters_inspects_only_awaited() {
        let pool = Box::pin(RawEventPool::<i32>::new());

        let (_sender1, receiver1) = unsafe { pool.as_ref().rent() };
        let (sender2, receiver2) = unsafe { pool.as_ref().rent() };
        let (_sender3, _receiver3) = unsafe { pool.as_ref().rent() };

        let mut receiver1 = Box::pin(receiver1);
        let mut receiver2 = Box::pin(receiver2);

        let mut cx = task::Context::from_waker(Waker::noop());
        _ = receiver1.as_mut().poll(&mut cx);
        _ = receiver2.as_mut().poll(&mut cx);

        let mut inspected_count = 0;

        pool.inspect_awaiters(|_bt| {
            inspected_count += 1;
        });

        assert_eq!(inspected_count, 2);

        drop(sender2);
        drop(receiver2);

        let mut inspected_count = 0;

        pool.inspect_awaiters(|_bt| {
            inspected_count += 1;
        });

        assert_eq!(inspected_count, 1);
    }

    #[test]
    fn default_creates_functional_pool() {
        let pool = Box::pin(RawEventPool::<i32>::default());

        assert!(pool.is_empty());

        let (sender, receiver) = unsafe { pool.as_ref().rent() };
        let mut receiver = Box::pin(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiters_propagates_panic_from_closure() {
        let pool = Box::pin(RawEventPool::<i32>::new());

        // SAFETY: The pool outlives both endpoints.
        let (_sender, receiver) = unsafe { pool.as_ref().rent() };
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());
        _ = receiver.as_mut().poll(&mut cx);

        assert_panics_with(
            || {
                pool.as_ref().inspect_awaiters(|_bt| {
                    panic!("intentional panic to verify pass-through");
                });
            },
            |message| assert!(message.contains("pass-through")),
        );

        // The pool is still usable, which proves that the panic did not leave any lock behind.
        assert_eq!(pool.len(), 1);

        let mut inspected_count = 0;

        pool.inspect_awaiters(|_bt| {
            inspected_count += 1;
        });

        assert_eq!(inspected_count, 1);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiters_closure_may_reenter_pool() {
        // The callback re-enters the pool, so a regression that calls it under the pool lock
        // deadlocks on the non-reentrant mutex. The watchdog turns that into a bounded failure.
        with_watchdog(|| {
            let pool = Box::pin(RawEventPool::<i32>::new());

            // SAFETY: The pool outlives both endpoints.
            let (_sender, receiver) = unsafe { pool.as_ref().rent() };
            let mut receiver = Box::pin(receiver);

            let mut cx = task::Context::from_waker(Waker::noop());
            _ = receiver.as_mut().poll(&mut cx);

            assert_inspect_awaiters_is_reentrant(&|f| pool.inspect_awaiters(f), &|| {
                // SAFETY: The pool outlives both endpoints.
                let (sender, receiver) = unsafe { pool.as_ref().rent() };
                drop(sender);
                drop(receiver);
            });
        });
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiters_tolerates_endpoint_drop_from_closure() {
        const EVENT_COUNT: usize = 3;

        // Dropping endpoints from the callback returns events to the pool, which takes the pool
        // lock. The watchdog bounds the deadlock that a callback-under-lock regression causes.
        with_watchdog(|| {
            let pool = Box::pin(RawEventPool::<i32>::new());

            let mut cx = task::Context::from_waker(Waker::noop());

            let mut endpoints = Vec::with_capacity(EVENT_COUNT);

            for _ in 0..EVENT_COUNT {
                // SAFETY: The pool outlives both endpoints.
                let (sender, receiver) = unsafe { pool.as_ref().rent() };
                let mut receiver = Box::pin(receiver);
                _ = receiver.as_mut().poll(&mut cx);
                endpoints.push((sender, receiver));
            }

            // The closure releases the events it is inspecting. The backtraces it receives are
            // snapshots, so they remain valid and each event is still visited exactly once.
            let endpoints = RefCell::new(endpoints);
            let mut inspected_count = 0;

            pool.inspect_awaiters(|_bt| {
                inspected_count += 1;
                drop(endpoints.borrow_mut().pop());
            });

            assert_eq!(inspected_count, EVENT_COUNT);
            assert!(pool.is_empty());
        });
    }

    #[cfg(debug_assertions)]
    #[test]
    fn released_event_releases_backtrace() {
        let pool = Box::pin(RawEventPool::<i32>::new());

        // SAFETY: The pool outlives both endpoints.
        let (sender, receiver) = unsafe { pool.as_ref().rent() };
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());
        _ = receiver.as_mut().poll(&mut cx);

        // The receiver leaves the event behind for the sender to release.
        drop(receiver);

        let mut backtraces = pool.awaiter_backtraces();
        assert_eq!(backtraces.len(), 1);

        let backtrace = backtraces.pop().expect("the event has been awaited");
        assert_eq!(Arc::strong_count(&backtrace), 2);

        drop(sender);

        // Releasing the event releases its backtrace, leaving the snapshot as the only owner.
        assert_eq!(Arc::strong_count(&backtrace), 1);
    }
}
