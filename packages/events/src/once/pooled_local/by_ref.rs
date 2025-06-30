//! Reference-based pooled local event endpoints.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use pinned_pool::Key;

use super::LocalOnceEventPool;

/// A sender endpoint for pooled local events that holds a reference to the pool.
///
/// This sender is created from [`LocalOnceEventPool::bind_by_ref`] and automatically manages
/// the lifetime of the underlying event. When both sender and receiver are dropped,
/// the event is automatically returned to the pool.
///
/// This is the single-threaded variant that cannot be sent across threads.
#[derive(Debug)]
pub struct ByRefPooledLocalOnceSender<'a, T> {
    pub(super) pool: &'a LocalOnceEventPool<T>,
    pub(super) key: Key,
}

impl<T> ByRefPooledLocalOnceSender<'_, T> {
    /// Sends a value through the event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::LocalOnceEventPool;
    ///
    /// let pool = LocalOnceEventPool::new();
    /// let (sender, receiver) = pool.bind_by_ref();
    ///
    /// sender.send(42);
    /// let value = futures::executor::block_on(receiver).unwrap();
    /// assert_eq!(value, 42);
    /// ```
    pub fn send(self, value: T) {
        // TODO: There should not be any "get" here - we should have a
        // direct reference to the event.
        let inner_pool = self.pool.pool.borrow();
        let item = inner_pool.get(self.key);
        let event = item.get().get_ref();

        event.set(value);
    }
}

impl<T> Drop for ByRefPooledLocalOnceSender<'_, T> {
    fn drop(&mut self) {
        self.pool.dec_ref_and_cleanup(self.key);
    }
}

/// A receiver endpoint for pooled local events that holds a reference to the pool.
///
/// This receiver is created from [`LocalOnceEventPool::bind_by_ref`] and automatically manages
/// the lifetime of the underlying event. When both sender and receiver are dropped,
/// the event is automatically returned to the pool.
///
/// This is the single-threaded variant that cannot be sent across threads.
#[derive(Debug)]
pub struct ByRefPooledLocalOnceReceiver<'a, T> {
    pub(super) pool: &'a LocalOnceEventPool<T>,
    pub(super) key: Option<Key>,
}

impl<T> Future for ByRefPooledLocalOnceReceiver<'_, T> {
    type Output = Result<T, crate::disconnected::Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // Check if this receiver has already been consumed
        let Some(key) = this.key else {
            panic!("ByRefPooledLocalOnceReceiver already consumed")
        };

        // Get the event from the pool and poll it
        let inner_pool = this.pool.pool.borrow();
        let item = inner_pool.get(key);
        let event = item.get().get_ref();
        let poll_result = event.poll(cx.waker());

        if let Some(value) = poll_result {
            // Release the borrow before cleanup
            drop(inner_pool);

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

impl<T> Drop for ByRefPooledLocalOnceReceiver<'_, T> {
    fn drop(&mut self) {
        // Clean up our reference if not consumed by Future
        if let Some(key) = self.key {
            self.pool.dec_ref_and_cleanup(key);
        }
    }
}
