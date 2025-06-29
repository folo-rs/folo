//! Rc-based senders and receivers for pooled events.

use std::rc::Rc;
use std::ptr::NonNull;

use pinned_pool::Key;

use super::{Event, EventPool};

/// A sender that sends values through pooled thread-safe events using Rc ownership.
#[derive(Debug)]
pub struct ByRcPooledEventSender<T>
where
    T: Send,
{
    pub(super) pool: Rc<std::cell::RefCell<EventPool<T>>>,
    pub(super) key: Key,
}

impl<T> ByRcPooledEventSender<T>
where
    T: Send,
{
    /// Sends a value through the pooled event.
    pub fn send(self, value: T) {
        // Get the pool item first
        let pool_borrowed = self.pool.borrow();
        let item = pool_borrowed.pool.get(self.key);

        // SAFETY: The event pointer is valid as long as we hold a reference in the pool
        let event: &Event<T> = unsafe { NonNull::from(item.get()).as_ref() };
        drop(event.try_set(value));

        // Drop the borrow before cleanup
        drop(pool_borrowed);

        // Clean up our reference
        self.pool.borrow_mut().dec_ref_and_cleanup(self.key);

        // Prevent double cleanup in Drop
        std::mem::forget(self);
    }
}

impl<T> Drop for ByRcPooledEventSender<T>
where
    T: Send,
{
    fn drop(&mut self) {
        // Clean up our reference if not consumed by send()
        self.pool.borrow_mut().dec_ref_and_cleanup(self.key);
    }
}

/// A receiver that receives values from pooled thread-safe events using Rc ownership.
#[derive(Debug)]
pub struct ByRcPooledEventReceiver<T>
where
    T: Send,
{
    pub(super) pool: Rc<std::cell::RefCell<EventPool<T>>>,
    pub(super) key: Key,
}

impl<T> ByRcPooledEventReceiver<T>
where
    T: Send,
{
    /// Receives a value from the pooled event.
    #[must_use]
    pub fn recv(self) -> T {
        // Get the pool item first
        let pool_borrowed = self.pool.borrow();
        let item = pool_borrowed.pool.get(self.key);

        // SAFETY: The event pointer is valid as long as we hold a reference in the pool
        let event: &Event<T> = unsafe { NonNull::from(item.get()).as_ref() };
        let result = futures::executor::block_on(crate::futures::EventFuture::new(event));

        // Drop the borrow before cleanup
        drop(pool_borrowed);

        // Clean up our reference
        self.pool.borrow_mut().dec_ref_and_cleanup(self.key);

        // Prevent double cleanup in Drop
        std::mem::forget(self);

        result
    }

    /// Receives a value from the pooled event asynchronously.
    pub async fn recv_async(self) -> T {
        // Get the event pointer without holding a borrow across await
        let event_ptr = {
            let pool_borrowed = self.pool.borrow();
            let item = pool_borrowed.pool.get(self.key);
            NonNull::from(item.get())
        };

        // SAFETY: The event pointer is valid as long as we hold a reference in the pool
        let event: &Event<T> = unsafe { event_ptr.as_ref() };
        let result = crate::futures::EventFuture::new(event).await;

        // Clean up our reference
        self.pool.borrow_mut().dec_ref_and_cleanup(self.key);

        // Prevent double cleanup in Drop
        std::mem::forget(self);

        result
    }
}

impl<T> Drop for ByRcPooledEventReceiver<T>
where
    T: Send,
{
    fn drop(&mut self) {
        // Clean up our reference if not consumed by recv()
        self.pool.borrow_mut().dec_ref_and_cleanup(self.key);
    }
}
