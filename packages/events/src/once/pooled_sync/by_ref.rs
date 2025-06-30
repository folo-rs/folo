//! Reference-based senders and receivers for pooled events.

use std::marker::PhantomData;

use pinned_pool::Key;

use super::OnceEventPool;

/// A sender that sends values through pooled thread-safe events using reference to pool.
#[derive(Debug)]
pub struct ByRefPooledOnceSender<'p, T>
where
    T: Send,
{
    pub(super) pool: *const OnceEventPool<T>,
    pub(super) key: Key,
    pub(super) _phantom: PhantomData<&'p OnceEventPool<T>>,
}

impl<T> ByRefPooledOnceSender<'_, T>
where
    T: Send,
{
    /// Returns the key associated with this sender's event.
    #[must_use]
    pub fn key(&self) -> Key {
        self.key
    }

    /// Sends a value through the pooled event.
    pub fn send(self, value: T) {
        // SAFETY: The pool pointer is valid for the lifetime of this struct
        let pool = unsafe { &*self.pool };

        // Get the event from the pool
        let pool_guard = pool.pool.lock().expect("pool mutex should not be poisoned");
        let item = pool_guard.get(self.key);
        let event = item.get().get_ref();

        drop(event.try_set(value));
    }
}

impl<T> Drop for ByRefPooledOnceSender<'_, T>
where
    T: Send,
{
    fn drop(&mut self) {
        // SAFETY: The pool pointer is valid for the lifetime of this struct
        let pool = unsafe { &*self.pool };
        pool.dec_ref_and_cleanup(self.key);
    }
}

/// A receiver that receives values from pooled thread-safe events using reference to pool.
#[derive(Debug)]
pub struct ByRefPooledOnceReceiver<'p, T>
where
    T: Send,
{
    pub(super) pool: *const OnceEventPool<T>,
    pub(super) key: Key,
    pub(super) _phantom: PhantomData<&'p OnceEventPool<T>>,
}

impl<T> ByRefPooledOnceReceiver<'_, T>
where
    T: Send,
{
    /// Receives a value from the pooled event asynchronously.
    pub async fn recv_async(self) -> Result<T, crate::disconnected::Disconnected> {
        // SAFETY: The pool pointer is valid for the lifetime of this struct
        let pool = unsafe { &*self.pool };

        // Get the event pointer without holding a lock across await
        let event_ptr = {
            let pool_guard = pool.pool.lock().expect("pool mutex should not be poisoned");
            let item = pool_guard.get(self.key);
            let event = item.get().get_ref();
            std::ptr::NonNull::from(event)
        };

        // SAFETY: The event is guaranteed to remain valid until we clean it up
        let event = unsafe { event_ptr.as_ref() };
        crate::futures::EventFuture::new(event).await
    }
}

impl<T> Drop for ByRefPooledOnceReceiver<'_, T>
where
    T: Send,
{
    fn drop(&mut self) {
        // SAFETY: The pool pointer is valid for the lifetime of this struct
        let pool = unsafe { &*self.pool };
        pool.dec_ref_and_cleanup(self.key);
    }
}
