use std::any::type_name;
#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::sync::Arc;

use infinity_pool::RawPinnedPool;
use parking_lot::Mutex;

use crate::{Event, PooledReceiver, PooledRef, PooledSender, ReceiverCore, SenderCore};

/// A pool of reusable one-time thread-safe events.
///
/// # Examples
///
/// ```
/// use events_once::EventPool;
///
/// # #[tokio::main]
/// # async fn main() {
/// let pool = EventPool::<String>::new();
///
/// for i in 0..3 {
///     let (tx, rx) = pool.rent();
///
///     tx.send(format!("Message {i}"));
///
///     let message = rx.await.unwrap();
///     println!("{message}");
/// }
/// # }
/// ```
pub struct EventPool<T: Send + 'static> {
    core: Arc<EventPoolCore<T>>,

    _owns_some: PhantomData<T>,
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: Send + 'static> fmt::Debug for EventPool<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("core", &self.core)
            .finish()
    }
}

pub(crate) struct EventPoolCore<T: Send + 'static> {
    pub(crate) pool: Mutex<RawPinnedPool<UnsafeCell<MaybeUninit<Event<T>>>>>,
}

impl<T: Send + 'static> EventPool<T> {
    /// Creates a new empty event pool.
    #[must_use]
    pub fn new() -> Self {
        Self {
            core: Arc::new(EventPoolCore {
                pool: Mutex::new(RawPinnedPool::new()),
            }),
            _owns_some: PhantomData,
        }
    }

    /// Rents an event from the pool, returning its endpoints.
    ///
    /// The event will be returned to the pool when both endpoints are dropped.
    #[must_use]
    pub fn rent(&self) -> (PooledSender<T>, PooledReceiver<T>) {
        let storage = {
            let mut pool = self.core.pool.lock();

            #[expect(
                clippy::multiple_unsafe_ops_per_block,
                unused_unsafe,
                reason = "it cannot handle the closure"
            )]
            // SAFETY: We are required to initialize the storage of the item we store in the pool.
            // We do - that is what new_in_inner is for.
            unsafe {
                pool.insert_with(|place| {
                    // This is a sandwich of MaybeUninit<UnsafeCell<MaybeUninit<Event<T>>>>.
                    // The outer MaybeUninit is for the pool to manage uninitialized storage.
                    // It does not know that we are expecting to use the internal MaybeUninit
                    // instead (which we want to do to preserve the UnsafeCell around everything).
                    //
                    // SAFETY: We still treat it as uninitialized due to the inner MaybeUninit.
                    let place = unsafe { place.assume_init_mut() };

                    Event::new_in_inner(place);
                })
            }
        }
        .into_shared();

        let event_ref = PooledRef::new(Arc::clone(&self.core), storage);

        let inner_sender = SenderCore::new(event_ref.clone());
        let inner_receiver = ReceiverCore::new(event_ref);

        (
            PooledSender::new(inner_sender),
            PooledReceiver::new(inner_receiver),
        )
    }

    /// Returns `true` if no events have currently been rented from the pool.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        let pool = self.core.pool.lock();

        pool.is_empty()
    }

    /// Returns the number of events that have currently been rented from the pool.
    #[must_use]
    pub fn len(&self) -> usize {
        let pool = self.core.pool.lock();

        pool.len()
    }

    /// Uses the provided closure to inspect the backtraces of the most recent awaiter of each
    /// awaited event in the pool.
    ///
    /// This method is only available in debug builds (`cfg(debug_assertions)`).
    /// For any data to be present, `RUST_BACKTRACE=1` or `RUST_LIB_BACKTRACE=1` must be set.
    ///
    /// The closure is called once for each event in the pool that has been awaited at some point
    /// in the past.
    #[cfg(debug_assertions)]
    pub fn inspect_awaiters(&self, mut f: impl FnMut(&Backtrace)) {
        let pool = self.core.pool.lock();

        for event_ptr in pool.iter() {
            // SAFETY: The pool remains alive for the duration of this function call, satisfying
            // the lifetime requirement. The pointer is valid as it comes from the pool's iterator.
            // We only ever create shared references to the events, so no conflicting exclusive
            // references can exist.
            let event_cell = unsafe { event_ptr.as_ref() };

            // SAFETY: See above.
            let event_maybe = unsafe { event_cell.get().as_ref() };

            // SAFETY: UnsafeCell pointer is never null.
            let event = unsafe { event_maybe.unwrap_unchecked() };

            // SAFETY: We only ever create shared references, never exclusive ones.
            let event = unsafe { event.assume_init_ref() };

            event.inspect_awaiter(|bt| {
                if let Some(bt) = bt {
                    f(bt);
                }
            });
        }
    }
}

impl<T: Send + 'static> Default for EventPool<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Send + 'static> Clone for EventPool<T> {
    fn clone(&self) -> Self {
        Self {
            core: Arc::clone(&self.core),
            _owns_some: PhantomData,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: Send + 'static> fmt::Debug for EventPoolCore<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("pool", &self.pool)
            .finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::Barrier;
    use std::task::{self, Poll, Waker};
    use std::{iter, thread};

    use futures::executor::block_on;
    use static_assertions::assert_impl_all;
    use testing::with_watchdog;

    use super::*;
    use crate::Disconnected;

    assert_impl_all!(EventPool<u32>: Send, Sync);

    #[test]
    fn len() {
        let pool = EventPool::<i32>::new();

        assert_eq!(pool.len(), 0);

        let (sender1, receiver1) = pool.rent();
        assert_eq!(pool.len(), 1);

        let (sender2, receiver2) = pool.rent();
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
        let pool = EventPool::<i32>::new();

        assert!(pool.is_empty());

        let (sender, receiver) = pool.rent();

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

        let pool = EventPool::<i32>::new();

        assert!(pool.is_empty());

        for _ in 0..ITERATIONS {
            let (sender, receiver) = pool.rent();
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

        let pool = EventPool::<i32>::new();

        for _ in 0..ITERATIONS {
            let endpoints = iter::repeat_with(|| pool.rent())
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
        let pool = EventPool::<i32>::new();

        let (sender, _) = pool.rent();

        sender.send(42);
    }

    #[test]
    fn drop_receive() {
        let pool = EventPool::<i32>::new();

        let (_, receiver) = pool.rent();
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn receive_drop_receive() {
        let pool = EventPool::<i32>::new();

        let (sender, receiver) = pool.rent();
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
        let pool = EventPool::<i32>::new();

        let (sender, receiver) = pool.rent();
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(receiver);

        sender.send(42);
    }

    #[test]
    fn receive_drop_drop_receiver_first() {
        let pool = EventPool::<i32>::new();

        let (sender, receiver) = pool.rent();
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(receiver);
        drop(sender);
    }

    #[test]
    fn receive_drop_drop_sender_first() {
        let pool = EventPool::<i32>::new();

        let (sender, receiver) = pool.rent();
        let mut receiver = Box::pin(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(sender);
        drop(receiver);
    }

    #[test]
    fn drop_drop_receiver_first() {
        let pool = EventPool::<i32>::new();

        let (sender, receiver) = pool.rent();

        drop(receiver);
        drop(sender);
    }

    #[test]
    fn drop_drop_sender_first() {
        let pool = EventPool::<i32>::new();

        let (sender, receiver) = pool.rent();

        drop(sender);
        drop(receiver);
    }

    #[test]
    fn is_ready() {
        let pool = EventPool::<i32>::new();

        let (sender, receiver) = pool.rent();
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
        let pool = EventPool::<i32>::new();

        let (sender, receiver) = pool.rent();
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
        let pool = EventPool::<i32>::new();

        let (sender, receiver) = pool.rent();

        let Err(crate::IntoValueError::Pending(receiver)) = receiver.into_value() else {
            panic!("Expected receiver to not be ready");
        };

        sender.send(42);

        assert!(matches!(receiver.into_value(), Ok(42)));
    }

    #[test]
    #[should_panic]
    fn panic_poll_after_completion() {
        let pool = EventPool::<i32>::new();

        let (sender, receiver) = pool.rent();
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
        let pool = EventPool::<i32>::new();

        let (sender, receiver) = pool.rent();
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
            let pool = EventPool::<i32>::new();

            let (sender, receiver) = pool.rent();

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
            let pool = EventPool::<i32>::new();

            let (sender, receiver) = pool.rent();

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
            let pool = EventPool::<i32>::new();

            let (sender, receiver) = pool.rent();

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
            let pool = EventPool::<i32>::new();

            let (sender, receiver) = pool.rent();

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
            let pool = EventPool::<i32>::new();

            let (sender, receiver) = pool.rent();

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

    #[test]
    fn drop_pool_send_receive() {
        let pool = EventPool::<i32>::new();

        let (sender, receiver) = pool.rent();

        drop(pool);

        let mut receiver = Box::pin(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiters_inspects_only_awaited() {
        let pool = EventPool::<i32>::new();

        let (_sender1, receiver1) = pool.rent();
        let (sender2, receiver2) = pool.rent();
        let (_sender3, _receiver3) = pool.rent();

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

    #[cfg(debug_assertions)]
    #[test]
    fn clones_are_equivalent() {
        let pool1 = EventPool::<i32>::new();
        let pool2 = pool1.clone();

        let (_sender1, receiver1) = pool1.rent();
        let (_sender2, receiver2) = pool2.rent();

        let mut cx = task::Context::from_waker(Waker::noop());

        let mut receiver1 = Box::pin(receiver1);
        let mut receiver2 = Box::pin(receiver2);

        _ = receiver1.as_mut().poll(&mut cx);
        _ = receiver2.as_mut().poll(&mut cx);

        // The inspect_awaiters() logic is sticky, so we can use that to validate.
        let mut inspected_count = 0;

        pool1.inspect_awaiters(|_bt| {
            inspected_count += 1;
        });

        assert_eq!(inspected_count, 2);

        let mut inspected_count = 0;

        pool2.inspect_awaiters(|_bt| {
            inspected_count += 1;
        });

        assert_eq!(inspected_count, 2);
    }

    #[test]
    fn default_creates_functional_pool() {
        let pool = EventPool::<i32>::default();

        assert!(pool.is_empty());

        let (sender, receiver) = pool.rent();
        let mut receiver = Box::pin(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }
}
