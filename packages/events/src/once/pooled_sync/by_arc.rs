//! Arc-based senders and receivers for pooled events.

use std::ptr::NonNull;
use std::sync::Arc;

use pinned_pool::Key;

use super::{Event, EventPool};

/// A sender that sends values through pooled thread-safe events using Arc ownership.
#[derive(Debug)]
pub struct ByArcPooledEventSender<T>
where
    T: Send,
{
    pub(super) pool: Arc<EventPool<T>>,
    pub(super) key: Key,
}

// SAFETY: ByArcPooledEventSender can be Send as long as T: Send, since we only
// send the value T across threads, and the Arc and NonNull pointers are used to access
// the thread-safe Event<T> and EventPool<T>.
unsafe impl<T> Send for ByArcPooledEventSender<T> where T: Send {}

// SAFETY: ByArcPooledEventSender can be Sync as long as T: Send, since the
// Event<T> and EventPool<T> it points to are thread-safe.
unsafe impl<T> Sync for ByArcPooledEventSender<T> where T: Send {}

impl<T> ByArcPooledEventSender<T>
where
    T: Send,
{
    /// Sends a value through the pooled event.
    pub fn send(self, value: T) {
        // Get the pool item first
        let pool_locked = self
            .pool
            .pool
            .lock()
            .expect("pool mutex should not be poisoned");
        let item = pool_locked.get(self.key);

        // Get the event reference from the pinned wrapper
        let event: &Event<T> = item.get().get_ref();
        drop(event.try_set(value));
    }
}

impl<T> Drop for ByArcPooledEventSender<T>
where
    T: Send,
{
    fn drop(&mut self) {
        self.pool.dec_ref_and_cleanup(self.key);
    }
}

/// A receiver that receives values from pooled thread-safe events using Arc ownership.
#[derive(Debug)]
pub struct ByArcPooledEventReceiver<T>
where
    T: Send,
{
    pub(super) pool: Arc<EventPool<T>>,
    pub(super) key: Key,
}

// SAFETY: ByArcPooledEventReceiver can be Send as long as T: Send, since we only
// send the value T across threads, and the Arc and NonNull pointers are used to access
// the thread-safe Event<T> and EventPool<T>.
unsafe impl<T> Send for ByArcPooledEventReceiver<T> where T: Send {}

// SAFETY: ByArcPooledEventReceiver can be Sync as long as T: Send, since the
// Event<T> and EventPool<T> it points to are thread-safe.
unsafe impl<T> Sync for ByArcPooledEventReceiver<T> where T: Send {}

impl<T> ByArcPooledEventReceiver<T>
where
    T: Send,
{
    /// Receives a value from the pooled event asynchronously.
    pub async fn recv_async(self) -> T {
        // Get the event pointer without holding a lock across await
        let event_ptr = {
            let pool_locked = self
                .pool
                .pool
                .lock()
                .expect("pool mutex should not be poisoned");
            let item = pool_locked.get(self.key);
            NonNull::from(item.get().get_ref())
        };

        // SAFETY: The event pointer is valid as long as we hold a reference in the pool
        let event: &Event<T> = unsafe { event_ptr.as_ref() };
        crate::futures::EventFuture::new(event).await
    }
}

impl<T> Drop for ByArcPooledEventReceiver<T>
where
    T: Send,
{
    fn drop(&mut self) {
        self.pool.dec_ref_and_cleanup(self.key);
    }
}
