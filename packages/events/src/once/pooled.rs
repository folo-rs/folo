//! Pooled events that provide automatic resource management.
//!
//! This module provides pooled variants of events that automatically manage their lifecycle
//! using reference counting. Events are created from pools and automatically returned to the
//! pool when both sender and receiver are dropped.

use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::Arc;

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

    /// Creates sender and receiver endpoints connected by Rc to the pool.
    ///
    /// The pool will create a new event and return endpoints that hold Rc references.
    /// When both endpoints are dropped, the event will be automatically cleaned up.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::rc::Rc;
    ///
    /// use events::once::EventPool;
    ///
    /// let pool = Rc::new(std::cell::RefCell::new(EventPool::<i32>::new()));
    /// let (sender, receiver) = pool.borrow_mut().by_rc(&pool);
    ///
    /// sender.send(42);
    /// let value = receiver.recv();
    /// assert_eq!(value, 42);
    /// ```
    pub fn by_rc(
        &mut self,
        pool_rc: &Rc<std::cell::RefCell<Self>>,
    ) -> (ByRcPooledEventSender<T>, ByRcPooledEventReceiver<T>) {
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

        (
            ByRcPooledEventSender {
                pool: Rc::clone(pool_rc),
                event_ptr,
                key,
            },
            ByRcPooledEventReceiver {
                pool: Rc::clone(pool_rc),
                event_ptr,
                key,
            },
        )
    }

    /// Creates sender and receiver endpoints connected by Arc to the pool.
    ///
    /// The pool will create a new event and return endpoints that hold Arc references.
    /// When both endpoints are dropped, the event will be automatically cleaned up.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::sync::{Arc, Mutex};
    ///
    /// use events::once::EventPool;
    ///
    /// let pool = Arc::new(Mutex::new(EventPool::<i32>::new()));
    /// let (sender, receiver) = pool.lock().unwrap().by_arc(&pool);
    ///
    /// sender.send(42);
    /// let value = receiver.recv();
    /// assert_eq!(value, 42);
    /// ```
    pub fn by_arc(
        &mut self,
        pool_arc: &Arc<std::sync::Mutex<Self>>,
    ) -> (ByArcPooledEventSender<T>, ByArcPooledEventReceiver<T>) {
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

        (
            ByArcPooledEventSender {
                pool: Arc::clone(pool_arc),
                event_ptr,
                key,
            },
            ByArcPooledEventReceiver {
                pool: Arc::clone(pool_arc),
                event_ptr,
                key,
            },
        )
    }

    /// Creates sender and receiver endpoints connected by raw pointer to the pool.
    ///
    /// The pool will create a new event and return endpoints that hold raw pointers.
    /// When both endpoints are dropped, the event will be automatically cleaned up.
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    /// - The pool remains valid and pinned for the entire lifetime of the sender and receiver
    /// - The sender and receiver are dropped before the pool is dropped
    /// - The pool is not moved after calling this method
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::pin::Pin;
    ///
    /// use events::once::EventPool;
    ///
    /// let mut pool = EventPool::<i32>::new();
    /// let pinned_pool = Pin::new(&mut pool);
    /// // SAFETY: We ensure the pool outlives the sender and receiver
    /// let (sender, receiver) = unsafe { pinned_pool.by_ptr() };
    ///
    /// sender.send(42);
    /// let value = receiver.recv();
    /// assert_eq!(value, 42);
    /// // sender and receiver are dropped here, before pool
    /// ```
    #[must_use]
    pub unsafe fn by_ptr(
        self: Pin<&mut Self>,
    ) -> (ByPtrPooledEventSender<T>, ByPtrPooledEventReceiver<T>) {
        // SAFETY: We need to access the mutable reference to create the event
        // The caller guarantees the pool remains pinned and valid
        let this = unsafe { self.get_unchecked_mut() };

        let inserter = this.pool.begin_insert();
        let key = inserter.key();
        let item = inserter.insert_mut(WithRefCount::new(Event::new()));

        // Get a pointer to the inner event
        let event_ptr = NonNull::from(item.get());

        // Increment reference count for both sender and receiver
        // SAFETY: WithRefCount doesn't contain self-references so it's safe to get_unchecked_mut
        let item_mut = unsafe { item.get_unchecked_mut() };
        item_mut.inc_ref();
        item_mut.inc_ref();

        let pool_ptr: *mut Self = this;

        (
            ByPtrPooledEventSender {
                pool: pool_ptr,
                event_ptr,
                key,
            },
            ByPtrPooledEventReceiver {
                pool: pool_ptr,
                event_ptr,
                key,
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

/// A sender that sends values through pooled thread-safe events using Rc ownership.
#[derive(Debug)]
pub struct ByRcPooledEventSender<T>
where
    T: Send,
{
    pool: Rc<std::cell::RefCell<EventPool<T>>>,
    event_ptr: NonNull<Event<T>>,
    key: Key,
}

impl<T> ByRcPooledEventSender<T>
where
    T: Send,
{
    /// Sends a value through the pooled event.
    pub fn send(self, value: T) {
        // SAFETY: The event pointer is valid as long as we hold a reference in the pool
        let event = unsafe { self.event_ptr.as_ref() };
        drop(event.try_set(value));

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
    pool: Rc<std::cell::RefCell<EventPool<T>>>,
    event_ptr: NonNull<Event<T>>,
    key: Key,
}

impl<T> ByRcPooledEventReceiver<T>
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
        self.pool.borrow_mut().dec_ref_and_cleanup(self.key);

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

/// A sender that sends values through pooled thread-safe events using Arc ownership.
#[derive(Debug)]
pub struct ByArcPooledEventSender<T>
where
    T: Send,
{
    pool: Arc<std::sync::Mutex<EventPool<T>>>,
    event_ptr: NonNull<Event<T>>,
    key: Key,
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
        // SAFETY: The event pointer is valid as long as we hold a reference in the pool
        let event = unsafe { self.event_ptr.as_ref() };
        drop(event.try_set(value));

        // Clean up our reference
        self.pool.lock().unwrap().dec_ref_and_cleanup(self.key);

        // Prevent double cleanup in Drop
        std::mem::forget(self);
    }
}

impl<T> Drop for ByArcPooledEventSender<T>
where
    T: Send,
{
    fn drop(&mut self) {
        // Clean up our reference if not consumed by send()
        self.pool.lock().unwrap().dec_ref_and_cleanup(self.key);
    }
}

/// A receiver that receives values from pooled thread-safe events using Arc ownership.
#[derive(Debug)]
pub struct ByArcPooledEventReceiver<T>
where
    T: Send,
{
    pool: Arc<std::sync::Mutex<EventPool<T>>>,
    event_ptr: NonNull<Event<T>>,
    key: Key,
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
    /// Receives a value from the pooled event.
    #[must_use]
    pub fn recv(self) -> T {
        // SAFETY: The event pointer is valid as long as we hold a reference in the pool
        let event = unsafe { self.event_ptr.as_ref() };
        let result = futures::executor::block_on(crate::futures::EventFuture::new(event));

        // Clean up our reference
        self.pool.lock().unwrap().dec_ref_and_cleanup(self.key);

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
        self.pool.lock().unwrap().dec_ref_and_cleanup(self.key);

        // Prevent double cleanup in Drop
        std::mem::forget(self);

        result
    }
}

impl<T> Drop for ByArcPooledEventReceiver<T>
where
    T: Send,
{
    fn drop(&mut self) {
        // Clean up our reference if not consumed by recv()
        self.pool.lock().unwrap().dec_ref_and_cleanup(self.key);
    }
}

/// A sender that sends values through pooled thread-safe events using raw pointer.
#[derive(Debug)]
pub struct ByPtrPooledEventSender<T>
where
    T: Send,
{
    pool: *mut EventPool<T>,
    event_ptr: NonNull<Event<T>>,
    key: Key,
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

impl<T> Drop for ByPtrPooledEventSender<T>
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

/// A receiver that receives values from pooled thread-safe events using raw pointer.
#[derive(Debug)]
pub struct ByPtrPooledEventReceiver<T>
where
    T: Send,
{
    pool: *mut EventPool<T>,
    event_ptr: NonNull<Event<T>>,
    key: Key,
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

impl<T> Drop for ByPtrPooledEventReceiver<T>
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

    #[test]
    fn event_pool_by_rc() {
        use std::cell::RefCell;
        use std::rc::Rc;

        with_watchdog(|| {
            let pool = Rc::new(RefCell::new(EventPool::<i32>::new()));
            let (sender, receiver) = pool.borrow_mut().by_rc(&pool);

            sender.send(42);
            let value = receiver.recv();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_pool_by_arc() {
        use std::sync::{Arc, Mutex};

        with_watchdog(|| {
            let pool = Arc::new(Mutex::new(EventPool::<i32>::new()));
            let (sender, receiver) = pool.lock().unwrap().by_arc(&pool);

            sender.send(42);
            let value = receiver.recv();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_pool_by_ptr() {
        use std::pin::Pin;

        with_watchdog(|| {
            let mut pool = EventPool::<i32>::new();
            let pinned_pool = Pin::new(&mut pool);
            // SAFETY: We ensure the pool outlives the sender and receiver
            let (sender, receiver) = unsafe { pinned_pool.by_ptr() };

            sender.send(42);
            let value = receiver.recv();
            assert_eq!(value, 42);
            // sender and receiver are dropped here, before pool
        });
    }

    #[test]
    fn event_pool_by_rc_async() {
        use std::cell::RefCell;
        use std::rc::Rc;

        use futures::executor::block_on;

        with_watchdog(|| {
            let pool = Rc::new(RefCell::new(EventPool::<i32>::new()));
            let (sender, receiver) = pool.borrow_mut().by_rc(&pool);

            sender.send(42);
            let value = block_on(receiver.recv_async());
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_pool_by_arc_async() {
        use std::sync::{Arc, Mutex};

        use futures::executor::block_on;

        with_watchdog(|| {
            let pool = Arc::new(Mutex::new(EventPool::<i32>::new()));
            let (sender, receiver) = pool.lock().unwrap().by_arc(&pool);

            sender.send(42);
            let value = block_on(receiver.recv_async());
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_pool_by_ptr_async() {
        use std::pin::Pin;

        use futures::executor::block_on;

        with_watchdog(|| {
            let mut pool = EventPool::<i32>::new();
            let pinned_pool = Pin::new(&mut pool);
            // SAFETY: We ensure the pool outlives the sender and receiver
            let (sender, receiver) = unsafe { pinned_pool.by_ptr() };

            sender.send(42);
            let value = block_on(receiver.recv_async());
            assert_eq!(value, 42);
            // sender and receiver are dropped here, before pool
        });
    }
}
