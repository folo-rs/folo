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
    }
}

impl<T> Drop for ByRefPooledEventSender<'_, T>
where
    T: Send,
{
    fn drop(&mut self) {
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
    /// Receives a value from the pooled event asynchronously.
    pub async fn recv_async(self) -> T {
        // SAFETY: The pool pointer is valid for the lifetime of this struct
        let pool = unsafe { &mut *self.pool };

        // Get the event from the pool
        let item = pool.pool.get(self.key);
        let event = item.get();

        crate::futures::EventFuture::new(event).await
    }
}

impl<T> Drop for ByRefPooledEventReceiver<'_, T>
where
    T: Send,
{
    fn drop(&mut self) {
        // SAFETY: The pool pointer is valid for the lifetime of this struct
        let pool = unsafe { &mut *self.pool };
        pool.dec_ref_and_cleanup(self.key);
    }
}
