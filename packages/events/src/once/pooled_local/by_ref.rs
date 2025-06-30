//! Reference-based pooled local event endpoints.

use std::marker::PhantomData;

use pinned_pool::Key;

use super::LocalEventPool;

/// A sender endpoint for pooled local events that holds a reference to the pool.
///
/// This sender is created from [`LocalEventPool::by_ref`] and automatically manages
/// the lifetime of the underlying event. When both sender and receiver are dropped,
/// the event is automatically returned to the pool.
///
/// This is the single-threaded variant that cannot be sent across threads.
#[derive(Debug)]
pub struct ByRefPooledLocalEventSender<'a, T> {
    pub(super) pool: *const LocalEventPool<T>,
    pub(super) key: Key,
    pub(super) _phantom: PhantomData<&'a LocalEventPool<T>>,
}

impl<T> ByRefPooledLocalEventSender<'_, T> {
    /// Sends a value through the event.
    ///
    /// If there is a receiver waiting, it will be woken up. If the receiver has been
    /// dropped, the value is stored and will be returned when a new receiver attempts
    /// to receive.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::LocalEventPool;
    ///
    /// let pool = LocalEventPool::new();
    /// let (sender, receiver) = pool.by_ref();
    ///
    /// sender.send(42);
    /// let value = futures::executor::block_on(receiver);
    /// assert_eq!(value, 42);
    /// ```
    pub fn send(self, value: T) {
        // SAFETY: Pool is guaranteed to be valid by the lifetime parameter
        let pool = unsafe { &*self.pool };

        // Get the event from the pool
        let pool_borrow = pool.pool.borrow();
        let item = pool_borrow.get(self.key);
        let event = item.get();

        drop(event.try_set(value));
    }
}

impl<T> Drop for ByRefPooledLocalEventSender<'_, T> {
    fn drop(&mut self) {
        // SAFETY: Pool is guaranteed to be valid by the lifetime parameter
        let pool = unsafe { &*self.pool };
        pool.dec_ref_and_cleanup(self.key);
    }
}

/// A receiver endpoint for pooled local events that holds a reference to the pool.
///
/// This receiver is created from [`LocalEventPool::by_ref`] and automatically manages
/// the lifetime of the underlying event. When both sender and receiver are dropped,
/// the event is automatically returned to the pool.
///
/// This is the single-threaded variant that cannot be sent across threads.
#[derive(Debug)]
pub struct ByRefPooledLocalEventReceiver<'a, T> {
    pub(super) pool: *const LocalEventPool<T>,
    pub(super) key: Option<Key>,
    pub(super) _phantom: PhantomData<&'a LocalEventPool<T>>,
}

impl<T> ByRefPooledLocalEventReceiver<'_, T> {
    // This receiver can be awaited directly as it implements Future
}

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

impl<T> Future for ByRefPooledLocalEventReceiver<'_, T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // Check if this receiver has already been consumed
        let Some(key) = this.key else {
            panic!("ByRefPooledLocalEventReceiver already consumed")
        };

        // SAFETY: Pool is guaranteed to be valid by the lifetime parameter
        let pool = unsafe { &*this.pool };

        // Get the event from the pool and poll it
        let pool_borrow = pool.pool.borrow();
        let item = pool_borrow.get(key);
        let event = item.get();
        let poll_result = event.poll_recv(cx.waker());

        if let Some(value) = poll_result {
            // Release the borrow before cleanup
            drop(pool_borrow);

            // We got the value, clean up and return
            pool.dec_ref_and_cleanup(key);

            // Mark this receiver as consumed
            this.key = None;

            Poll::Ready(value)
        } else {
            Poll::Pending
        }
    }
}

impl<T> Drop for ByRefPooledLocalEventReceiver<'_, T> {
    fn drop(&mut self) {
        // Clean up our reference if not consumed by Future
        if let Some(key) = self.key {
            // SAFETY: Pool is guaranteed to be valid by the lifetime parameter
            let pool = unsafe { &*self.pool };
            pool.dec_ref_and_cleanup(key);
        }
    }
}
