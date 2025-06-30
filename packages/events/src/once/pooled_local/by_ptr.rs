//! Raw pointer-based pooled local event endpoints.

use pinned_pool::Key;

use super::LocalOnceEventPool;

/// A sender endpoint for pooled local events that holds a raw pointer to the pool.
///
/// This sender is created from [`LocalOnceEventPool::bind_by_ptr`] and automatically manages
/// the lifetime of the underlying event. When both sender and receiver are dropped,
/// the event is automatically returned to the pool.
///
/// This is the single-threaded variant that cannot be sent across threads.
///
/// # Safety
///
/// This type holds a raw pointer to the pool and requires that:
/// - The pool remains valid and pinned for the entire lifetime of this sender
/// - This sender is dropped before the pool is dropped
/// - The pool is not moved after creating this sender
#[derive(Debug)]
pub struct ByPtrPooledLocalOnceSender<T> {
    pub(super) pool: *const LocalOnceEventPool<T>,
    pub(super) key: Key,
}

impl<T> ByPtrPooledLocalOnceSender<T> {
    /// Sends a value through the event.
    ///
    /// If there is a receiver waiting, it will be woken up. If the receiver has been
    /// dropped, the value is stored and will be returned when a new receiver attempts
    /// to receive.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::pin::Pin;
    ///
    /// use events::once::LocalOnceEventPool;
    ///
    /// let pool = LocalOnceEventPool::new();
    /// let pinned_pool = Pin::new(&pool);
    /// // SAFETY: We ensure the pool outlives the sender and receiver
    /// let (sender, receiver) = unsafe { pinned_pool.bind_by_ptr() };
    ///
    /// sender.send(42);
    /// let value = futures::executor::block_on(receiver);
    /// assert_eq!(value, 42);
    /// ```
    pub fn send(self, value: T) {
        // SAFETY: Caller guarantees pool is valid for the lifetime of this sender
        let pool = unsafe { &*self.pool };

        // Get the event from the pool
        let pool_borrow = pool.pool.borrow();
        let item = pool_borrow.get(self.key);
        let event = item.get().get_ref();

        drop(event.try_set(value));
    }
}

impl<T> Drop for ByPtrPooledLocalOnceSender<T> {
    fn drop(&mut self) {
        // SAFETY: Caller guarantees pool is valid for the lifetime of this sender
        let pool = unsafe { &*self.pool };
        pool.dec_ref_and_cleanup(self.key);
    }
}

/// A receiver endpoint for pooled local events that holds a raw pointer to the pool.
///
/// This receiver is created from [`LocalOnceEventPool::bind_by_ptr`] and automatically manages
/// the lifetime of the underlying event. When both sender and receiver are dropped,
/// the event is automatically returned to the pool.
///
/// This is the single-threaded variant that cannot be sent across threads.
///
/// # Safety
///
/// This type holds a raw pointer to the pool and requires that:
/// - The pool remains valid and pinned for the entire lifetime of this receiver
/// - This receiver is dropped before the pool is dropped
/// - The pool is not moved after creating this receiver
#[derive(Debug)]
pub struct ByPtrPooledLocalOnceReceiver<T> {
    pub(super) pool: *const LocalOnceEventPool<T>,
    pub(super) key: Option<Key>,
}

impl<T> ByPtrPooledLocalOnceReceiver<T> {
    // This receiver can be awaited directly as it implements Future
}

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

impl<T> Future for ByPtrPooledLocalOnceReceiver<T> {
    type Output = Result<T, crate::disconnected::Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();

        // Check if this receiver has already been consumed
        let Some(key) = this.key else {
            panic!("ByPtrPooledLocalOnceReceiver already consumed")
        };

        // SAFETY: Caller guarantees pool is valid for the lifetime of this receiver
        let pool = unsafe { &*this.pool };

        // Get the event from the pool and poll it
        let pool_borrow = pool.pool.borrow();
        let item = pool_borrow.get(key);
        let event = item.get().get_ref();
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

impl<T> Drop for ByPtrPooledLocalOnceReceiver<T> {
    fn drop(&mut self) {
        // Clean up our reference if not consumed by Future
        if let Some(key) = self.key {
            // SAFETY: Caller guarantees pool is valid for the lifetime of this receiver
            let pool = unsafe { &*self.pool };
            pool.dec_ref_and_cleanup(key);
        }
    }
}
