//! Raw pointer-based senders and receivers for pooled events.

use std::ptr::NonNull;

use pinned_pool::Key;

use super::{Event, EventPool};

/// A sender that sends values through pooled thread-safe events using raw pointer.
#[derive(Debug)]
pub struct ByPtrPooledEventSender<T>
where
    T: Send,
{
    pub(super) pool: *mut EventPool<T>,
    pub(super) key: Key,
}

// SAFETY: ByPtrPooledEventSender can be Send as long as T: Send, since we only
// send the value T across threads, and the pointers are used to access
// the thread-safe Event<T> and EventPool<T>.
unsafe impl<T> Send for ByPtrPooledEventSender<T> where T: Send {}

// SAFETY: ByPtrPooledEventSender can be Sync as long as T: Send, since the
// Event<T> and EventPool<T> it points to are thread-safe.
unsafe impl<T> Sync for ByPtrPooledEventSender<T> where T: Send {}

impl<T> ByPtrPooledEventSender<T>
where
    T: Send,
{
    /// Sends a value through the pooled event.
    pub fn send(self, value: T) {
        // Get the pool item first
        // SAFETY: The pool pointer is valid for the lifetime of this struct
        let pool = unsafe { &*self.pool };
        let item = pool.pool.get(self.key);

        // SAFETY: The event pointer is valid as long as we hold a reference in the pool
        let event: &Event<T> = unsafe { NonNull::from(item.get()).as_ref() };
        drop(event.try_set(value));
    }
}

impl<T> Drop for ByPtrPooledEventSender<T>
where
    T: Send,
{
    fn drop(&mut self) {
        // SAFETY: The pool pointer is valid for the lifetime of this struct
        let pool = unsafe { &mut *self.pool };
        pool.dec_ref_and_cleanup(self.key);
    }
}

/// A receiver that receives values from pooled thread-safe events using raw pointer.
#[derive(Debug)]
pub struct ByPtrPooledEventReceiver<T>
where
    T: Send,
{
    pub(super) pool: *mut EventPool<T>,
    pub(super) key: Key,
}

// SAFETY: ByPtrPooledEventReceiver can be Send as long as T: Send, since we only
// send the value T across threads, and the pointers are used to access
// the thread-safe Event<T> and EventPool<T>.
unsafe impl<T> Send for ByPtrPooledEventReceiver<T> where T: Send {}

// SAFETY: ByPtrPooledEventReceiver can be Sync as long as T: Send, since the
// Event<T> and EventPool<T> it points to are thread-safe.
unsafe impl<T> Sync for ByPtrPooledEventReceiver<T> where T: Send {}

impl<T> ByPtrPooledEventReceiver<T>
where
    T: Send,
{
    /// Receives a value from the pooled event.
    #[must_use]
    pub fn recv(self) -> T {
        // Get the pool item first
        // SAFETY: The pool pointer is valid for the lifetime of this struct
        let pool = unsafe { &*self.pool };
        let item = pool.pool.get(self.key);

        // SAFETY: The event pointer is valid as long as we hold a reference in the pool
        let event: &Event<T> = unsafe { NonNull::from(item.get()).as_ref() };
        futures::executor::block_on(crate::futures::EventFuture::new(event))
    }

    /// Receives a value from the pooled event asynchronously.
    pub async fn recv_async(self) -> T {
        // Get the event pointer first
        let event_ptr = {
            // SAFETY: The pool pointer is valid for the lifetime of this struct
            let pool = unsafe { &*self.pool };
            let item = pool.pool.get(self.key);
            NonNull::from(item.get())
        };

        // SAFETY: The event pointer is valid as long as we hold a reference in the pool
        let event: &Event<T> = unsafe { event_ptr.as_ref() };
        crate::futures::EventFuture::new(event).await
    }
}

impl<T> Drop for ByPtrPooledEventReceiver<T>
where
    T: Send,
{
    fn drop(&mut self) {
        // SAFETY: The pool pointer is valid for the lifetime of this struct
        let pool = unsafe { &mut *self.pool };
        pool.dec_ref_and_cleanup(self.key);
    }
}
