use std::any::type_name;
#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
use std::cell::{RefCell, UnsafeCell};
use std::fmt;
use std::marker::PhantomData;
use std::pin::Pin;
use std::ptr::NonNull;

use infinity_pool::RawPinnedPool;

use crate::{
    LocalEvent, LocalReceiverCore, LocalSenderCore, RawLocalPooledReceiver, RawLocalPooledRef,
    RawLocalPooledSender,
};

/// A pool of reusable single-threaded one-time events with manual pool lifecycle management.
///
/// # Examples
///
/// ```
/// use events_once::RawLocalEventPool;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let pool = Box::pin(RawLocalEventPool::<String>::new());
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
pub struct RawLocalEventPool<T: 'static> {
    // This is in an UnsafeCell to logically "detach" it from the parent object.
    // We will create direct (shared) references to the contents of the cell not only from
    // the pool but also from the event references themselves. This is safe as long as
    // we never create conflicting references. We could not guarantee that for the parent
    // object but we can guarantee it for the cell contents.
    core: NonNull<UnsafeCell<RawLocalEventPoolCore<T>>>,

    _owns_some: PhantomData<T>,
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: 'static> fmt::Debug for RawLocalEventPool<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("core", &self.core)
            .finish()
    }
}

impl<T: 'static> Drop for RawLocalEventPool<T> {
    #[cfg_attr(test, mutants::skip)] // Impractical to test deallocation - Miri will complain if we leak.
    fn drop(&mut self) {
        // SAFETY: We are the owner of the core, so we know it remains valid.
        // Anyone calling rent() has to promise that we outlive the rented event
        // which means that we must be the last remaining user of the core.
        drop(unsafe { Box::from_raw(self.core.as_ptr()) });
    }
}

pub(crate) struct RawLocalEventPoolCore<T: 'static> {
    pub(crate) pool: RefCell<RawPinnedPool<LocalEvent<T>>>,
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: 'static> fmt::Debug for RawLocalEventPoolCore<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("pool", &self.pool)
            .finish()
    }
}

impl<T: 'static> RawLocalEventPool<T> {
    /// Creates a new empty event pool.
    #[must_use]
    pub fn new() -> Self {
        let core = RawLocalEventPoolCore {
            pool: RefCell::new(RawPinnedPool::new()),
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
    pub unsafe fn rent(self: Pin<&Self>) -> (RawLocalPooledSender<T>, RawLocalPooledReceiver<T>) {
        let storage = {
            // SAFETY: We are the owner of the core, so we know it remains valid. We only ever
            // create shared references to it, so no conflicting exclusive references can exist.
            let core_cell = unsafe { self.core.as_ref() };

            // SAFETY: See above.
            let core_maybe = unsafe { core_cell.get().as_ref() };

            // SAFETY: UnsafeCell pointer is never null.
            let core = unsafe { core_maybe.unwrap_unchecked() };

            let mut pool = core.pool.borrow_mut();

            // SAFETY: We are required to initialize the storage of the item we store in the pool.
            // We do - that is what new_in_inner is for.
            unsafe {
                pool.insert_with(|place| {
                    LocalEvent::new_in_inner(place);
                })
            }
        }
        .into_shared();

        let event_ref = RawLocalPooledRef::new(self.core, storage);

        let inner_sender = LocalSenderCore::new(event_ref.clone());
        let inner_receiver = LocalReceiverCore::new(event_ref);

        (
            RawLocalPooledSender::new(inner_sender),
            RawLocalPooledReceiver::new(inner_receiver),
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

        let pool = core.pool.borrow();

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

        let pool = core.pool.borrow();

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

        let pool = core.pool.borrow();

        for event_ptr in pool.iter() {
            // SAFETY: The pool remains alive for the duration of this function call, satisfying
            // the lifetime requirement. The pointer is valid as it comes from the pool's iterator.
            // We only ever create shared references to the events, so no conflicting exclusive
            // references can exist.
            let event = unsafe { event_ptr.as_ref() };

            event.inspect_awaiter(|bt| {
                if let Some(bt) = bt {
                    f(bt);
                }
            });
        }
    }
}

impl<T: 'static> Default for RawLocalEventPool<T> {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: The pool is thread-safe - the only reason it does not have it via auto traits is that
// we have the NonNUll pointer that disables thread safety auto traits. However, all the logic is
// actually protected via the core Mutex, so all is well.
unsafe impl<T: 'static> Send for RawLocalEventPool<T> {}
// SAFETY: See above.
unsafe impl<T: 'static> Sync for RawLocalEventPool<T> {}

#[cfg(test)]
#[allow(clippy::undocumented_unsafe_blocks, reason = "test code, be concise")]
mod tests {
    use std::iter;
    use std::pin::pin;
    use std::task::{self, Poll, Waker};

    use static_assertions::assert_impl_all;

    use super::*;
    use crate::Disconnected;

    assert_impl_all!(RawLocalEventPool<u32>: Send, Sync);

    #[test]
    fn len() {
        let pool = pin!(RawLocalEventPool::<i32>::new());

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
        let pool = pin!(RawLocalEventPool::<i32>::new());

        assert!(pool.is_empty());

        let (sender, receiver) = unsafe { pool.as_ref().rent() };

        assert!(!pool.is_empty());

        {
            let mut receiver = pin!(receiver);

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

        let pool = pin!(RawLocalEventPool::<i32>::new());

        assert!(pool.is_empty());

        for _ in 0..ITERATIONS {
            let (sender, receiver) = unsafe { pool.as_ref().rent() };
            let mut receiver = pin!(receiver);

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

        let pool = pin!(RawLocalEventPool::<i32>::new());

        for _ in 0..ITERATIONS {
            let endpoints = iter::repeat_with(|| unsafe { pool.as_ref().rent() })
                .take(BATCH_SIZE)
                .collect::<Vec<_>>();

            for (sender, receiver) in endpoints {
                let mut receiver = pin!(receiver);

                sender.send(42);

                let mut cx = task::Context::from_waker(Waker::noop());

                let poll_result = receiver.as_mut().poll(&mut cx);
                assert!(matches!(poll_result, Poll::Ready(Ok(42))));
            }
        }
    }

    #[test]
    fn drop_send() {
        let pool = pin!(RawLocalEventPool::<i32>::new());

        let (sender, _) = unsafe { pool.as_ref().rent() };

        sender.send(42);
    }

    #[test]
    fn drop_receive() {
        let pool = pin!(RawLocalEventPool::<i32>::new());

        let (_, receiver) = unsafe { pool.as_ref().rent() };
        let mut receiver = pin!(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn receive_drop_receive() {
        let pool = pin!(RawLocalEventPool::<i32>::new());

        let (sender, receiver) = unsafe { pool.as_ref().rent() };
        let mut receiver = pin!(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Pending));

        drop(sender);

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn receive_drop_send() {
        let pool = pin!(RawLocalEventPool::<i32>::new());

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
        let pool = pin!(RawLocalEventPool::<i32>::new());

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
        let pool = pin!(RawLocalEventPool::<i32>::new());

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
        let pool = pin!(RawLocalEventPool::<i32>::new());

        let (sender, receiver) = unsafe { pool.as_ref().rent() };

        drop(receiver);
        drop(sender);
    }

    #[test]
    fn drop_drop_sender_first() {
        let pool = pin!(RawLocalEventPool::<i32>::new());

        let (sender, receiver) = unsafe { pool.as_ref().rent() };

        drop(sender);
        drop(receiver);
    }

    #[test]
    fn is_ready() {
        let pool = pin!(RawLocalEventPool::<i32>::new());

        let (sender, receiver) = unsafe { pool.as_ref().rent() };
        let mut receiver = pin!(receiver);

        assert!(!receiver.is_ready());

        sender.send(42);

        assert!(receiver.is_ready());

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn drop_is_ready() {
        let pool = pin!(RawLocalEventPool::<i32>::new());

        let (sender, receiver) = unsafe { pool.as_ref().rent() };
        let mut receiver = pin!(receiver);

        assert!(!receiver.is_ready());

        drop(sender);

        assert!(receiver.is_ready());

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn into_value() {
        let pool = pin!(RawLocalEventPool::<i32>::new());

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
        let pool = pin!(RawLocalEventPool::<i32>::new());

        let (sender, receiver) = unsafe { pool.as_ref().rent() };
        let mut receiver = pin!(receiver);

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
        let pool = pin!(RawLocalEventPool::<i32>::new());

        let (sender, receiver) = unsafe { pool.as_ref().rent() };
        let mut receiver = pin!(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        assert!(matches!(
            receiver.as_mut().poll(&mut cx),
            Poll::Ready(Ok(42))
        ));

        _ = receiver.is_ready();
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiters_inspects_only_awaited() {
        let pool = pin!(RawLocalEventPool::<i32>::new());

        let (_sender1, receiver1) = unsafe { pool.as_ref().rent() };
        let (sender2, receiver2) = unsafe { pool.as_ref().rent() };
        let (_sender3, _receiver3) = unsafe { pool.as_ref().rent() };

        let mut receiver1 = pin!(receiver1);
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
}
