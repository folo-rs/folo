//! Reference-based senders and receivers for pooled events.

use std::marker::PhantomData;

use pinned_pool::Key;

use super::EventPool;

/// A sender that sends values through pooled thread-safe events using reference to pool.
#[derive(Debug)]
pub struct ByRefPooledEventSender<'p, T>
where
    T: Send,
{
    pub(super) pool: *mut EventPool<T>,
    pub(super) key: Key,
    pub(super) _phantom: PhantomData<&'p mut EventPool<T>>,
}

impl<T> ByRefPooledEventSender<'_, T>
where
    T: Send,
{
    /// Sends a value through the pooled event.
    pub fn send(self, value: T) {
        // SAFETY: The pool pointer is valid for the lifetime of this struct
        let pool = unsafe { &mut *self.pool };

        // Get the event from the pool
        let item = pool.pool.get(self.key);
        let event = item.get();

        drop(event.try_set(value));

        // Clean up our reference
        pool.dec_ref_and_cleanup(self.key);

        // Prevent double cleanup in Drop
        std::mem::forget(self);
    }
}

impl<T> Drop for ByRefPooledEventSender<'_, T>
where
    T: Send,
{
    fn drop(&mut self) {
        // Clean up our reference if not consumed by send()
        // SAFETY: The pool pointer is valid for the lifetime of this struct
        let pool = unsafe { &mut *self.pool };
        pool.dec_ref_and_cleanup(self.key);
    }
}

/// A receiver that receives values from pooled thread-safe events using reference to pool.
#[derive(Debug)]
pub struct ByRefPooledEventReceiver<'p, T>
where
    T: Send,
{
    pub(super) pool: *mut EventPool<T>,
    pub(super) key: Key,
    pub(super) _phantom: PhantomData<&'p mut EventPool<T>>,
}

impl<T> ByRefPooledEventReceiver<'_, T>
where
    T: Send,
{
    /// Receives a value from the pooled event.
    #[must_use]
    pub fn recv(self) -> T {
        // SAFETY: The pool pointer is valid for the lifetime of this struct
        let pool = unsafe { &mut *self.pool };

        // Get the event from the pool
        let item = pool.pool.get(self.key);
        let event = item.get();

        let result = futures::executor::block_on(crate::futures::EventFuture::new(event));

        // Clean up our reference
        pool.dec_ref_and_cleanup(self.key);

        // Prevent double cleanup in Drop
        std::mem::forget(self);

        result
    }

    /// Receives a value from the pooled event asynchronously.
    pub async fn recv_async(self) -> T {
        // SAFETY: The pool pointer is valid for the lifetime of this struct
        let pool = unsafe { &mut *self.pool };

        // Get the event from the pool
        let item = pool.pool.get(self.key);
        let event = item.get();

        let result = crate::futures::EventFuture::new(event).await;

        // Clean up our reference
        pool.dec_ref_and_cleanup(self.key);

        // Prevent double cleanup in Drop
        std::mem::forget(self);

        result
    }
}

impl<T> Drop for ByRefPooledEventReceiver<'_, T>
where
    T: Send,
{
    fn drop(&mut self) {
        // Clean up our reference if not consumed by recv()
        // SAFETY: The pool pointer is valid for the lifetime of this struct
        let pool = unsafe { &mut *self.pool };
        pool.dec_ref_and_cleanup(self.key);
    }
}
