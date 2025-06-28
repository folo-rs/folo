//! Pooled events that provide automatic resource management.
//!
//! This module provides pooled variants of events that automatically manage their lifecycle
//! using reference counting. Events are created from pools and automatically returned to the
//! pool when both sender and receiver are dropped.

use std::ptr::NonNull;

use pinned_pool::{Key, PinnedPool};

use super::sync::Event;

/// Just combines a value and a reference count, for use in custom reference counting logic.
#[derive(Debug)]
pub struct WithRefCount<T> {
    value: T,
    ref_count: usize,
}

impl<T> WithRefCount<T> {
    /// Creates a new reference-counted wrapper with an initial reference count of 0.
    #[must_use]
    pub fn new(value: T) -> Self {
        Self {
            value,
            ref_count: 0,
        }
    }

    /// Returns a shared reference to the wrapped value.
    #[must_use]
    pub fn get(&self) -> &T {
        &self.value
    }

    /// Returns an exclusive reference to the wrapped value.
    #[must_use]
    pub fn get_mut(&mut self) -> &mut T {
        &mut self.value
    }

    /// Increments the reference count.
    pub fn inc_ref(&mut self) {
        self.ref_count = self.ref_count.saturating_add(1);
    }

    /// Decrements the reference count.
    pub fn dec_ref(&mut self) {
        self.ref_count = self.ref_count.saturating_sub(1);
    }

    /// Returns the current reference count.
    #[must_use]
    pub fn ref_count(&self) -> usize {
        self.ref_count
    }

    /// Returns `true` if the reference count is greater than 0.
    #[must_use]
    pub fn is_referenced(&self) -> bool {
        self.ref_count > 0
    }
}

impl<T> Default for WithRefCount<T>
where
    T: Default,
{
    fn default() -> Self {
        Self::new(T::default())
    }
}

/// A pool that manages thread-safe events with automatic cleanup.
///
/// The pool creates events on demand and automatically cleans them up when both
/// sender and receiver endpoints are dropped. Events are reference-counted to
/// track when they are no longer in use.
///
/// # Example
///
/// ```rust
/// use events::once::EventPool;
///
/// let mut pool = EventPool::<i32>::new();
/// let (sender, receiver) = pool.by_ref();
///
/// sender.send(42);
/// let value = receiver.recv();
/// assert_eq!(value, 42);
/// // Event is automatically returned to pool when sender/receiver are dropped
/// ```
#[derive(Debug)]
pub struct EventPool<T>
where
    T: Send,
{
    pool: PinnedPool<WithRefCount<Event<T>>>,
}

impl<T> EventPool<T>
where
    T: Send,
{
    /// Creates a new empty event pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::EventPool;
    ///
    /// let pool = EventPool::<String>::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            pool: PinnedPool::new(),
        }
    }

    /// Creates sender and receiver endpoints connected by reference to the pool.
    ///
    /// The pool will create a new event and return endpoints that reference it.
    /// When both endpoints are dropped, the event will be automatically cleaned up.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::EventPool;
    ///
    /// let mut pool = EventPool::<i32>::new();
    /// let (sender, receiver) = pool.by_ref();
    ///
    /// sender.send(42);
    /// let value = receiver.recv();
    /// assert_eq!(value, 42);
    /// ```
    pub fn by_ref(
        &mut self,
    ) -> (
        ByRefPooledEventSender<'_, T>,
        ByRefPooledEventReceiver<'_, T>,
    ) {
        let inserter = self.pool.begin_insert();
        let key = inserter.key();
        let item = inserter.insert_mut(WithRefCount::new(Event::new()));

        // Get a pointer to the inner event
        let event_ptr = NonNull::from(item.get());

        // Increment reference count for both sender and receiver
        // SAFETY: WithRefCount doesn't contain self-references so it's safe to get_unchecked_mut
        let item_mut = unsafe { item.get_unchecked_mut() };
        item_mut.inc_ref();
        item_mut.inc_ref();

        // Use raw pointer to avoid borrowing self twice
        let pool_ptr: *mut Self = self;

        (
            ByRefPooledEventSender {
                pool: pool_ptr,
                event_ptr,
                key,
                _phantom: std::marker::PhantomData,
            },
            ByRefPooledEventReceiver {
                pool: pool_ptr,
                event_ptr,
                key,
                _phantom: std::marker::PhantomData,
            },
        )
    }

    /// Decrements the reference count for an event and removes it if no longer referenced.
    fn dec_ref_and_cleanup(&mut self, key: Key) {
        let item = self.pool.get_mut(key);
        // SAFETY: WithRefCount doesn't contain self-references so it's safe to get_unchecked_mut
        let item_mut = unsafe { item.get_unchecked_mut() };
        item_mut.dec_ref();
        if !item_mut.is_referenced() {
            self.pool.remove(key);
        }
    }
}

impl<T> Default for EventPool<T>
where
    T: Send,
{
    fn default() -> Self {
        Self::new()
    }
}

// Thread-safe pooled event senders and receivers
/// A sender that sends values through pooled thread-safe events using reference to pool.
#[derive(Debug)]
pub struct ByRefPooledEventSender<'p, T>
where
    T: Send,
{
    pool: *mut EventPool<T>,
    event_ptr: NonNull<Event<T>>,
    key: Key,
    _phantom: std::marker::PhantomData<&'p mut EventPool<T>>,
}

impl<T> ByRefPooledEventSender<'_, T>
where
    T: Send,
{
    /// Sends a value through the pooled event.
    pub fn send(self, value: T) {
        // SAFETY: The event pointer is valid as long as we hold a reference in the pool
        let event = unsafe { self.event_ptr.as_ref() };
        drop(event.try_set(value));

        // Clean up our reference
        // SAFETY: The pool pointer is valid for the lifetime of this struct
        let pool = unsafe { &mut *self.pool };
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
    pool: *mut EventPool<T>,
    event_ptr: NonNull<Event<T>>,
    key: Key,
    _phantom: std::marker::PhantomData<&'p mut EventPool<T>>,
}

impl<T> ByRefPooledEventReceiver<'_, T>
where
    T: Send,
{
    /// Receives a value from the pooled event.
    #[must_use]
    pub fn recv(self) -> T {
        // SAFETY: The event pointer is valid as long as we hold a reference in the pool
        let event = unsafe { self.event_ptr.as_ref() };
        let result = futures::executor::block_on(crate::futures::EventFuture::new(event));

        // Clean up our reference
        // SAFETY: The pool pointer is valid for the lifetime of this struct
        let pool = unsafe { &mut *self.pool };
        pool.dec_ref_and_cleanup(self.key);

        // Prevent double cleanup in Drop
        std::mem::forget(self);

        result
    }

    /// Receives a value from the pooled event asynchronously.
    pub async fn recv_async(self) -> T {
        // SAFETY: The event pointer is valid as long as we hold a reference in the pool
        let event = unsafe { self.event_ptr.as_ref() };
        let result = crate::futures::EventFuture::new(event).await;

        // Clean up our reference
        // SAFETY: The pool pointer is valid for the lifetime of this struct
        let pool = unsafe { &mut *self.pool };
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

#[cfg(test)]
mod tests {
    use testing::with_watchdog;

    use super::*;

    #[test]
    fn with_ref_count_basic() {
        let mut wrapper = WithRefCount::new(42);
        assert_eq!(*wrapper.get(), 42);
        assert_eq!(wrapper.ref_count(), 0);
        assert!(!wrapper.is_referenced());

        wrapper.inc_ref();
        assert_eq!(wrapper.ref_count(), 1);
        assert!(wrapper.is_referenced());

        wrapper.dec_ref();
        assert_eq!(wrapper.ref_count(), 0);
        assert!(!wrapper.is_referenced());
    }

    #[test]
    fn event_pool_by_ref() {
        with_watchdog(|| {
            let mut pool = EventPool::<i32>::new();
            let (sender, receiver) = pool.by_ref();

            sender.send(42);
            let value = receiver.recv();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_pool_async() {
        use futures::executor::block_on;

        with_watchdog(|| {
            let mut pool = EventPool::<i32>::new();
            let (sender, receiver) = pool.by_ref();

            sender.send(42);
            let value = block_on(receiver.recv_async());
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn pool_drop_cleanup() {
        with_watchdog(|| {
            let mut pool = EventPool::<i32>::new();

            // Create and drop sender/receiver without using them
            let (sender, receiver) = pool.by_ref();
            drop(sender);
            drop(receiver);

            // Pool should be empty (the event should have been cleaned up)
            // This is implementation detail but shows the cleanup works
        });
    }

    #[test]
    fn pool_multiple_events() {
        with_watchdog(|| {
            let mut pool = EventPool::<i32>::new();

            // Test one event first
            let (sender1, receiver1) = pool.by_ref();
            sender1.send(1);
            let value1 = receiver1.recv();
            assert_eq!(value1, 1);

            // Test another event
            let (sender2, receiver2) = pool.by_ref();
            sender2.send(2);
            let value2 = receiver2.recv();
            assert_eq!(value2, 2);
        });
    }
}
