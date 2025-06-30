//! Reference-based senders and receivers for pooled events.

use std::ptr::NonNull;

use pinned_pool::Key;

use super::OnceEventPool;
use crate::futures::EventFuture;
use crate::{Disconnected, ERR_POISONED_LOCK};

/// A sender that sends values through pooled thread-safe events using reference to pool.
#[derive(Debug)]
pub struct ByRefPooledOnceSender<'p, T>
where
    T: Send,
{
    pub(super) pool: &'p OnceEventPool<T>,
    pub(super) key: Key,
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
        let inner_pool = self.pool.pool.lock().expect(ERR_POISONED_LOCK);
        let item = inner_pool.get(self.key);
        let event = item.get().get_ref();

        event.set(value);
    }
}

impl<T> Drop for ByRefPooledOnceSender<'_, T>
where
    T: Send,
{
    fn drop(&mut self) {
        self.pool.dec_ref_and_cleanup(self.key);
    }
}

/// A receiver that receives values from pooled thread-safe events using reference to pool.
#[derive(Debug)]
pub struct ByRefPooledOnceReceiver<'p, T>
where
    T: Send,
{
    pub(super) pool: &'p OnceEventPool<T>,
    pub(super) key: Key,
}

impl<T> ByRefPooledOnceReceiver<'_, T>
where
    T: Send,
{
    /// Receives a value from the pooled event asynchronously.
    pub async fn recv_async(self) -> Result<T, Disconnected> {
        // Get the event pointer without holding a lock across await
        let event_ptr = {
            let inner_pool = self.pool.pool.lock().expect(ERR_POISONED_LOCK);
            let item = inner_pool.get(self.key);
            let event = item.get().get_ref();
            NonNull::from(event)
        };

        // SAFETY: The event is guaranteed to remain valid until we clean it up
        let event = unsafe { event_ptr.as_ref() };
        EventFuture::new(event).await
    }
}

impl<T> Drop for ByRefPooledOnceReceiver<'_, T>
where
    T: Send,
{
    fn drop(&mut self) {
        self.pool.dec_ref_and_cleanup(self.key);
    }
}
