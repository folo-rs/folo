//! Raw pointer-based senders and receivers for pooled events.

use std::ptr::NonNull;

use pinned_pool::Key;

use super::{OnceEvent, OnceEventPool};

/// A sender that sends values through pooled thread-safe events using raw pointer.
#[derive(Debug)]
pub struct ByPtrPooledOnceSender<T>
where
    T: Send,
{
    pub(super) pool: *const OnceEventPool<T>,
    pub(super) key: Key,
}

// SAFETY: ByPtrPooledOnceSender can be Send as long as T: Send, since we only
// send the value T across threads, and the pointers are used to access
// the thread-safe OnceEvent<T> and OnceEventPool<T>.
unsafe impl<T> Send for ByPtrPooledOnceSender<T> where T: Send {}

// SAFETY: ByPtrPooledOnceSender can be Sync as long as T: Send, since the
// OnceEvent<T> and OnceEventPool<T> it points to are thread-safe.
unsafe impl<T> Sync for ByPtrPooledOnceSender<T> where T: Send {}

impl<T> ByPtrPooledOnceSender<T>
where
    T: Send,
{
    /// Sends a value through the pooled event.
    pub fn send(self, value: T) {
        // Get the pool item first
        // SAFETY: The pool pointer is valid for the lifetime of this struct
        let pool = unsafe { &*self.pool };
        let pool_guard = pool.pool.lock().expect("pool mutex should not be poisoned");
        let item = pool_guard.get(self.key);

        // Get the pinned event reference
        let event: &OnceEvent<T> = item.get().get_ref();
        drop(event.try_set(value));
    }
}

impl<T> Drop for ByPtrPooledOnceSender<T>
where
    T: Send,
{
    fn drop(&mut self) {
        // SAFETY: The pool pointer is valid for the lifetime of this struct
        let pool = unsafe { &*self.pool };
        pool.dec_ref_and_cleanup(self.key);
    }
}

/// A receiver that receives values from pooled thread-safe events using raw pointer.
#[derive(Debug)]
pub struct ByPtrPooledOnceReceiver<T>
where
    T: Send,
{
    pub(super) pool: *const OnceEventPool<T>,
    pub(super) key: Key,
}

// SAFETY: ByPtrPooledOnceReceiver can be Send as long as T: Send, since we only
// send the value T across threads, and the pointers are used to access
// the thread-safe OnceEvent<T> and OnceEventPool<T>.
unsafe impl<T> Send for ByPtrPooledOnceReceiver<T> where T: Send {}

// SAFETY: ByPtrPooledOnceReceiver can be Sync as long as T: Send, since the
// OnceEvent<T> and OnceEventPool<T> it points to are thread-safe.
unsafe impl<T> Sync for ByPtrPooledOnceReceiver<T> where T: Send {}

impl<T> ByPtrPooledOnceReceiver<T>
where
    T: Send,
{
    /// Receives a value from the pooled event asynchronously.
    pub async fn recv_async(self) -> Result<T, crate::disconnected::Disconnected> {
        // Get the event pointer first
        let event_ptr = {
            // SAFETY: The pool pointer is valid for the lifetime of this struct
            let pool = unsafe { &*self.pool };
            let pool_guard = pool.pool.lock().expect("pool mutex should not be poisoned");
            let item = pool_guard.get(self.key);
            NonNull::from(item.get().get_ref())
        };

        // SAFETY: The event pointer is valid as long as we hold a reference in the pool
        let event: &OnceEvent<T> = unsafe { event_ptr.as_ref() };
        crate::futures::EventFuture::new(event).await
    }
}

impl<T> Drop for ByPtrPooledOnceReceiver<T>
where
    T: Send,
{
    fn drop(&mut self) {
        // SAFETY: The pool pointer is valid for the lifetime of this struct
        let pool = unsafe { &*self.pool };
        pool.dec_ref_and_cleanup(self.key);
    }
}
