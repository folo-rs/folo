use std::any::type_name;
#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::ptr::NonNull;

use infinity_pool::RawPinnedPool;
use parking_lot::Mutex;

use crate::{Event, RawPooledReceiver, RawPooledRef, RawPooledSender, ReceiverCore, SenderCore};

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
pub struct RawEventPool<T: Send + 'static> {
    // This is in an UnsafeCell to logically "detach" it from the parent object.
    // We will create direct (shared) references to the contents of the cell not only from
    // the pool but also from the event references themselves. This is safe as long as
    // we never create conflicting references. We could not guarantee that for the parent
    // object but we can guarantee it for the cell contents.
    core: NonNull<UnsafeCell<RawEventPoolCore<T>>>,

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

impl<T: Send + 'static> Drop for RawEventPool<T> {
    #[cfg_attr(test, mutants::skip)] // Impractical to test deallocation - Miri will complain if we leak.
    fn drop(&mut self) {
        // SAFETY: We are the owner of the core, so we know it remains valid.
        // Anyone calling rent() has to promise that we outlive the rented event
        // which means that we must be the last remaining user of the core.
        drop(unsafe { Box::from_raw(self.core.as_ptr()) });
    }
}

pub(crate) struct RawEventPoolCore<T: Send + 'static> {
    pub(crate) pool: Mutex<RawPinnedPool<UnsafeCell<MaybeUninit<Event<T>>>>>,
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: Send + 'static> fmt::Debug for RawEventPoolCore<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("pool", &self.pool)
            .finish()
    }
}

impl<T: Send + 'static> RawEventPool<T> {
    /// Creates a new empty event pool.
    #[must_use]
    pub fn new() -> Self {
        let core = RawEventPoolCore {
            pool: Mutex::new(RawPinnedPool::new()),
        };

        let core_ptr = Box::into_raw(Box::new(UnsafeCell::new(core)));

        Self {
            // SAFETY: Boxed object is never null.
            core: unsafe { NonNull::new_unchecked(core_ptr) },
            _owns_some: PhantomData,
        }
    }

    /// Rents an event from the pool, returning its endpoints.
    ///
    /// The event will be returned to the pool when both endpoints are dropped.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the pool outlives the endpoints.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants tries a boatload of unviable mutations and wastes time on this.
    pub unsafe fn rent(self: Pin<&Self>) -> (RawPooledSender<T>, RawPooledReceiver<T>) {
        let storage = {
            // SAFETY: We are the owner of the core, so we know it remains valid. We only ever
            // create shared references to it, so no conflicting exclusive references can exist.
            let core_cell = unsafe { self.core.as_ref() };

            // SAFETY: See above.
            let core_maybe = unsafe { core_cell.get().as_ref() };

            // SAFETY: UnsafeCell pointer is never null.
            let core = unsafe { core_maybe.unwrap_unchecked() };

            let mut pool = core.pool.lock();

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

        let event_ref = RawPooledRef::new(self.core, storage);

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
        // SAFETY: We are the owner of the core, so we know it remains valid. We only ever
        // create shared references to it, so no conflicting exclusive references can exist.
        let core_cell = unsafe { self.core.as_ref() };

        // SAFETY: See above.
        let core_maybe = unsafe { core_cell.get().as_ref() };

        // SAFETY: UnsafeCell pointer is never null.
        let core = unsafe { core_maybe.unwrap_unchecked() };

        let pool = core.pool.lock();

        pool.is_empty()
    }

    /// Returns the number of events that have currently been rented from the pool.
    #[must_use]
    pub fn len(&self) -> usize {
        // SAFETY: We are the owner of the core, so we know it remains valid. We only ever
        // create shared references to it, so no conflicting exclusive references can exist.
        let core_cell = unsafe { self.core.as_ref() };

        // SAFETY: See above.
        let core_maybe = unsafe { core_cell.get().as_ref() };

        // SAFETY: UnsafeCell pointer is never null.
        let core = unsafe { core_maybe.unwrap_unchecked() };

        let pool = core.pool.lock();

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
        // SAFETY: We are the owner of the core, so we know it remains valid. We only ever
        // create shared references to it, so no conflicting exclusive references can exist.
        let core_cell = unsafe { self.core.as_ref() };

        // SAFETY: See above.
        let core_maybe = unsafe { core_cell.get().as_ref() };

        // SAFETY: UnsafeCell pointer is never null.
        let core = unsafe { core_maybe.unwrap_unchecked() };

        let pool = core.pool.lock();

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

impl<T: Send + 'static> Default for RawEventPool<T> {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: The pool is thread-safe - the only reason it does not have it via auto traits is that
// we have the NonNull pointer that disables thread safety auto traits. However, all the logic is
// actually protected via the core Mutex, so all is well.
unsafe impl<T: Send + 'static> Send for RawEventPool<T> {}
// SAFETY: See above.
unsafe impl<T: Send + 'static> Sync for RawEventPool<T> {}

#[cfg(test)]
#[allow(clippy::undocumented_unsafe_blocks, reason = "test code, be concise")]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::{Arc, Barrier};
    use std::task::{self, Poll, Waker};
    use std::{iter, thread};

    use spin_on::spin_on;
    use static_assertions::assert_impl_all;
    use testing::with_watchdog;

    use super::*;
    use crate::Disconnected;

    assert_impl_all!(RawEventPool<u32>: Send, Sync);

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
                spin_on(async {
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
                spin_on(async {
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
                spin_on(async {
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
}
