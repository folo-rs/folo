//! Rc-based pooled local event endpoints.

use std::rc::Rc;

use pinned_pool::Key;

use super::LocalEventPool;

/// A sender endpoint for pooled local events that holds an Rc to the pool.
///
/// This sender is created from [`LocalEventPool::by_rc`] and automatically manages
/// the lifetime of the underlying event. When both sender and receiver are dropped,
/// the event is automatically returned to the pool.
///
/// This is the single-threaded variant that cannot be sent across threads.
#[derive(Debug)]
pub struct ByRcPooledLocalEventSender<T> {
    pub(super) pool: Rc<LocalEventPool<T>>,
    pub(super) key: Key,
}

impl<T> ByRcPooledLocalEventSender<T> {
    /// Sends a value through the event.
    ///
    /// If there is a receiver waiting, it will be woken up. If the receiver has been
    /// dropped, the value is stored and will be returned when a new receiver attempts
    /// to receive.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::rc::Rc;
    ///
    /// use events::once::LocalEventPool;
    ///
    /// let pool = Rc::new(LocalEventPool::new());
    /// let (sender, receiver) = pool.by_rc(&pool);
    ///
    /// sender.send(42);
    /// let value = futures::executor::block_on(receiver);
    /// assert_eq!(value, 42);
    /// ```
    pub fn send(self, value: T) {
        // Get the event from the pool
        let pool_borrow = self.pool.pool.borrow();
        let item = pool_borrow.get(self.key);
        let event = item.get().get_ref();

        drop(event.try_set(value));
    }
}

impl<T> Drop for ByRcPooledLocalEventSender<T> {
    fn drop(&mut self) {
        self.pool.dec_ref_and_cleanup(self.key);
    }
}

/// A receiver endpoint for pooled local events that holds an Rc to the pool.
///
/// This receiver is created from [`LocalEventPool::by_rc`] and automatically manages
/// the lifetime of the underlying event. When both sender and receiver are dropped,
/// the event is automatically returned to the pool.
///
/// This is the single-threaded variant that cannot be sent across threads.
#[derive(Debug)]
pub struct ByRcPooledLocalEventReceiver<T> {
    pub(super) pool: Rc<LocalEventPool<T>>,
    pub(super) key: Option<Key>,
}

impl<T> ByRcPooledLocalEventReceiver<T> {
    // This receiver can be awaited directly as it implements Future
}

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

impl<T> Future for ByRcPooledLocalEventReceiver<T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // Check if this receiver has already been consumed
        let Some(key) = this.key else {
            panic!("ByRcPooledLocalEventReceiver already consumed")
        };

        // Get the event from the pool and poll it
        let poll_result = {
            let pool_borrow = this.pool.pool.borrow();
            let item = pool_borrow.get(key);
            let event = item.get().get_ref();
            event.poll_recv(cx.waker())
        };

        if let Some(value) = poll_result {
            // We got the value, clean up and return
            this.pool.dec_ref_and_cleanup(key);

            // Mark this receiver as consumed
            this.key = None;

            Poll::Ready(value)
        } else {
            Poll::Pending
        }
    }
}

impl<T> Drop for ByRcPooledLocalEventReceiver<T> {
    fn drop(&mut self) {
        // Clean up our reference if not consumed by Future
        if let Some(key) = self.key {
            self.pool.dec_ref_and_cleanup(key);
        }
    }
}
