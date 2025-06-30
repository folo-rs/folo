//! Rc-based senders and receivers for pooled events.

use std::ptr::NonNull;
use std::rc::Rc;

use pinned_pool::Key;

use super::{OnceEvent, OnceEventPool};
use crate::ERR_POISONED_LOCK;
use crate::futures::EventFuture;

/// A sender that sends values through pooled thread-safe events using Rc ownership.
#[derive(Debug)]
pub struct ByRcPooledOnceSender<T>
where
    T: Send,
{
    pub(super) pool: Rc<OnceEventPool<T>>,
    pub(super) key: Key,
}

impl<T> ByRcPooledOnceSender<T>
where
    T: Send,
{
    /// Sends a value through the pooled event.
    pub fn send(self, value: T) {
        let inner_pool = self.pool.pool.lock().expect(ERR_POISONED_LOCK);
        let item = inner_pool.get(self.key);

        // Get the event reference from the pinned wrapper
        let event: &OnceEvent<T> = item.get().get_ref();
        event.set(value);
    }
}

impl<T> Drop for ByRcPooledOnceSender<T>
where
    T: Send,
{
    fn drop(&mut self) {
        self.pool.dec_ref_and_cleanup(self.key);
    }
}

/// A receiver that receives values from pooled thread-safe events using Rc ownership.
#[derive(Debug)]
pub struct ByRcPooledOnceReceiver<T>
where
    T: Send,
{
    pub(super) pool: Rc<OnceEventPool<T>>,
    pub(super) key: Key,
}

impl<T> ByRcPooledOnceReceiver<T>
where
    T: Send,
{
    /// Receives a value from the pooled event asynchronously.
    pub async fn recv_async(self) -> Result<T, crate::disconnected::Disconnected> {
        // Get the event pointer without holding a borrow across await
        let event_ptr = {
            let inner_pool = self.pool.pool.lock().expect(ERR_POISONED_LOCK);
            let item = inner_pool.get(self.key);
            NonNull::from(item.get().get_ref())
        };

        // SAFETY: The event pointer is valid as long as we hold a reference to the pool
        let event: &OnceEvent<T> = unsafe { event_ptr.as_ref() };
        EventFuture::new(event).await
    }
}

impl<T> Drop for ByRcPooledOnceReceiver<T>
where
    T: Send,
{
    fn drop(&mut self) {
        self.pool.dec_ref_and_cleanup(self.key);
    }
}
