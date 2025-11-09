use std::any::type_name;
#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
use std::cell::RefCell;
use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;

use infinity_pool::RawPinnedPool;

use crate::{
    LocalEvent, LocalReceiverCore, LocalSenderCore, PooledLocalReceiver, PooledLocalRef,
    PooledLocalSender,
};

/// A pool of reusable one-time single-threaded events.
///
/// # Examples
///
/// ```
/// use events_once::LocalEventPool;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let pool = LocalEventPool::<String>::new();
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
pub struct LocalEventPool<T> {
    core: Rc<LocalPoolCore<T>>,

    _owns_some: PhantomData<T>,
}

impl<T> fmt::Debug for LocalEventPool<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("core", &self.core)
            .finish()
    }
}

pub(crate) struct LocalPoolCore<T> {
    pub(crate) pool: RefCell<RawPinnedPool<LocalEvent<T>>>,
}

impl<T> LocalEventPool<T> {
    /// Creates a new empty event pool.
    #[must_use]
    pub fn new() -> Self {
        Self {
            core: Rc::new(LocalPoolCore {
                pool: RefCell::new(RawPinnedPool::new()),
            }),
            _owns_some: PhantomData,
        }
    }

    /// Rents an event from the pool, returning its endpoints.
    ///
    /// The event will be returned to the pool when both endpoints are dropped.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants tries a boatload of unviable mutations and wastes time on this.
    pub fn rent(&self) -> (PooledLocalSender<T>, PooledLocalReceiver<T>) {
        let storage = {
            let mut pool = self.core.pool.borrow_mut();

            // SAFETY: We are required to initialize the storage of the item we store in the pool.
            // We do - that is what new_in_inner is for.
            unsafe {
                pool.insert_with(|place| {
                    LocalEvent::new_in_inner(place);
                })
            }
        }
        .into_shared();

        let event_ref = PooledLocalRef::new(Rc::clone(&self.core), storage);

        let inner_sender = LocalSenderCore::new(event_ref.clone());
        let inner_receiver = LocalReceiverCore::new(event_ref);

        (
            PooledLocalSender::new(inner_sender),
            PooledLocalReceiver::new(inner_receiver),
        )
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
        let pool = self.core.pool.borrow_mut();

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

impl<T> Default for LocalEventPool<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Clone for LocalEventPool<T> {
    fn clone(&self) -> Self {
        Self {
            core: Rc::clone(&self.core),
            _owns_some: PhantomData,
        }
    }
}

impl<T> fmt::Debug for LocalPoolCore<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("pool", &self.pool)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::iter;
    use std::pin::pin;
    use std::task::{self, Poll, Waker};

    use static_assertions::assert_not_impl_any;

    use super::*;
    use crate::Disconnected;

    assert_not_impl_any!(LocalEventPool<u32>: Send, Sync);

    #[test]
    fn send_receive() {
        let pool = LocalEventPool::<i32>::new();

        let (sender, receiver) = pool.rent();
        let mut receiver = pin!(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[test]
    fn send_receive_reused() {
        const ITERATIONS: usize = 32;

        let pool = LocalEventPool::<i32>::new();

        for _ in 0..ITERATIONS {
            let (sender, receiver) = pool.rent();
            let mut receiver = pin!(receiver);

            sender.send(42);

            let mut cx = task::Context::from_waker(Waker::noop());

            let poll_result = receiver.as_mut().poll(&mut cx);
            assert!(matches!(poll_result, Poll::Ready(Ok(42))));
        }
    }

    #[test]
    fn send_receive_reused_batches() {
        const ITERATIONS: usize = 4;
        const BATCH_SIZE: usize = 8;

        let pool = LocalEventPool::<i32>::new();

        for _ in 0..ITERATIONS {
            let endpoints = iter::repeat_with(|| pool.rent())
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
        let pool = LocalEventPool::<i32>::new();

        let (sender, _) = pool.rent();

        sender.send(42);
    }

    #[test]
    fn drop_receive() {
        let pool = LocalEventPool::<i32>::new();

        let (_, receiver) = pool.rent();
        let mut receiver = pin!(receiver);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Err(Disconnected))));
    }

    #[test]
    fn receive_drop_receive() {
        let pool = LocalEventPool::<i32>::new();

        let (sender, receiver) = pool.rent();
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
        let pool = LocalEventPool::<i32>::new();

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
        let pool = LocalEventPool::<i32>::new();

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
        let pool = LocalEventPool::<i32>::new();

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
        let pool = LocalEventPool::<i32>::new();

        let (sender, receiver) = pool.rent();

        drop(receiver);
        drop(sender);
    }

    #[test]
    fn drop_drop_sender_first() {
        let pool = LocalEventPool::<i32>::new();

        let (sender, receiver) = pool.rent();

        drop(sender);
        drop(receiver);
    }

    #[test]
    fn is_ready() {
        let pool = LocalEventPool::<i32>::new();

        let (sender, receiver) = pool.rent();
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
        let pool = LocalEventPool::<i32>::new();

        let (sender, receiver) = pool.rent();
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
        let pool = LocalEventPool::<i32>::new();

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
        let pool = LocalEventPool::<i32>::new();

        let (sender, receiver) = pool.rent();
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
        let pool = LocalEventPool::<i32>::new();

        let (sender, receiver) = pool.rent();
        let mut receiver = pin!(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        assert!(matches!(
            receiver.as_mut().poll(&mut cx),
            Poll::Ready(Ok(42))
        ));

        _ = receiver.is_ready();
    }

    #[test]
    fn drop_pool_send_receive() {
        let pool = LocalEventPool::<i32>::new();

        let (sender, receiver) = pool.rent();

        drop(pool);

        let mut receiver = pin!(receiver);

        sender.send(42);

        let mut cx = task::Context::from_waker(Waker::noop());

        let poll_result = receiver.as_mut().poll(&mut cx);
        assert!(matches!(poll_result, Poll::Ready(Ok(42))));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiters_inspects_only_awaited() {
        let pool = LocalEventPool::<i32>::new();

        let (_sender1, receiver1) = pool.rent();
        let (sender2, receiver2) = pool.rent();
        let (_sender3, _receiver3) = pool.rent();

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

    #[cfg(debug_assertions)]
    #[test]
    fn clones_are_equivalent() {
        let pool1 = LocalEventPool::<i32>::new();
        let pool2 = pool1.clone();

        let (_sender1, receiver1) = pool1.rent();
        let (_sender2, receiver2) = pool2.rent();

        let mut cx = task::Context::from_waker(Waker::noop());

        let mut receiver1 = pin!(receiver1);
        let mut receiver2 = pin!(receiver2);

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
}
