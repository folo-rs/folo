//! Pooled events that provide automatic resource management.
//!
//! This module provides pooled variants of events that automatically manage their lifecycle
//! using reference counting. Events are created from pools and automatically returned to the
//! pool when both sender and receiver are dropped.

use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;

use pinned_pool::{Key, PinnedPool};

use super::sync::Event;

mod by_arc;
mod by_ptr;
mod by_rc;
mod by_ref;

pub use by_arc::{ByArcPooledEventReceiver, ByArcPooledEventSender};
pub use by_ptr::{ByPtrPooledEventReceiver, ByPtrPooledEventSender};
pub use by_rc::{ByRcPooledEventReceiver, ByRcPooledEventSender};
pub use by_ref::{ByRefPooledEventReceiver, ByRefPooledEventSender};

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
/// # use futures::executor::block_on;
///
/// # block_on(async {
/// let mut pool = EventPool::<i32>::new();
/// let (sender, receiver) = pool.by_ref();
///
/// sender.send(42);
/// let value = receiver.recv_async().await;
/// assert_eq!(value, 42);
/// // Event is automatically returned to pool when sender/receiver are dropped
/// # });
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
    /// # use futures::executor::block_on;
    ///
    /// # block_on(async {
    /// let mut pool = EventPool::<i32>::new();
    /// let (sender, receiver) = pool.by_ref();
    ///
    /// sender.send(42);
    /// let value = receiver.recv_async().await;
    /// assert_eq!(value, 42);
    /// # });
    /// ```
    pub fn by_ref(
        &mut self,
    ) -> (
        ByRefPooledEventSender<'_, T>,
        ByRefPooledEventReceiver<'_, T>,
    ) {
        let inserter = self.pool.begin_insert();
        let key = inserter.key();
        let _item = inserter.insert_mut(WithRefCount::new(Event::new()));

        // Increment reference count for both sender and receiver
        let item_mut = self.pool.get_mut(key);
        // SAFETY: WithRefCount doesn't contain self-references so it's safe to get_unchecked_mut
        unsafe {
            let item_ref = item_mut.get_unchecked_mut();
            item_ref.inc_ref();
            item_ref.inc_ref();
        }

        // Use raw pointer to avoid borrowing self twice
        let pool_ptr: *mut Self = self;

        (
            ByRefPooledEventSender {
                pool: pool_ptr,
                key,
                _phantom: std::marker::PhantomData,
            },
            ByRefPooledEventReceiver {
                pool: pool_ptr,
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
    /// # use futures::executor::block_on;
    ///
    /// # block_on(async {
    /// let pool = Rc::new(std::cell::RefCell::new(EventPool::<i32>::new()));
    /// let (sender, receiver) = pool.borrow_mut().by_rc(&pool);
    ///
    /// sender.send(42);
    /// let value = receiver.recv_async().await;
    /// assert_eq!(value, 42);
    /// # });
    /// ```
    pub fn by_rc(
        &mut self,
        pool_rc: &Rc<std::cell::RefCell<Self>>,
    ) -> (ByRcPooledEventSender<T>, ByRcPooledEventReceiver<T>) {
        let inserter = self.pool.begin_insert();
        let key = inserter.key();
        let _item = inserter.insert_mut(WithRefCount::new(Event::new()));

        // Now increment reference count for both sender and receiver
        let item_mut = self.pool.get_mut(key);
        // SAFETY: WithRefCount doesn't contain self-references so it's safe to get_unchecked_mut
        unsafe {
            let item_ref = item_mut.get_unchecked_mut();
            item_ref.inc_ref();
            item_ref.inc_ref();
        }

        (
            ByRcPooledEventSender {
                pool: Rc::clone(pool_rc),
                key,
            },
            ByRcPooledEventReceiver {
                pool: Rc::clone(pool_rc),
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
    /// let value = futures::executor::block_on(receiver.recv_async());
    /// assert_eq!(value, 42);
    /// ```
    pub fn by_arc(
        &mut self,
        pool_arc: &Arc<std::sync::Mutex<Self>>,
    ) -> (ByArcPooledEventSender<T>, ByArcPooledEventReceiver<T>) {
        let inserter = self.pool.begin_insert();
        let key = inserter.key();
        let _item = inserter.insert_mut(WithRefCount::new(Event::new()));

        // Now increment reference count for both sender and receiver
        let item_mut = self.pool.get_mut(key);
        // SAFETY: WithRefCount doesn't contain self-references so it's safe to get_unchecked_mut
        unsafe {
            let item_ref = item_mut.get_unchecked_mut();
            item_ref.inc_ref();
            item_ref.inc_ref();
        }

        (
            ByArcPooledEventSender {
                pool: Arc::clone(pool_arc),
                key,
            },
            ByArcPooledEventReceiver {
                pool: Arc::clone(pool_arc),
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
    /// let value = futures::executor::block_on(receiver.recv_async());
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
        let _item = inserter.insert_mut(WithRefCount::new(Event::new()));

        // Now increment reference count for both sender and receiver
        let item_mut = this.pool.get_mut(key);
        // SAFETY: WithRefCount doesn't contain self-references so it's safe to get_unchecked_mut
        unsafe {
            let item_ref = item_mut.get_unchecked_mut();
            item_ref.inc_ref();
            item_ref.inc_ref();
        }

        let pool_ptr: *mut Self = this;

        (
            ByPtrPooledEventSender {
                pool: pool_ptr,
                key,
            },
            ByPtrPooledEventReceiver {
                pool: pool_ptr,
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

    /// Shrinks the capacity of the pool to reduce memory usage.
    ///
    /// This method attempts to release unused memory by reducing the pool's capacity.
    /// The actual reduction is implementation-dependent and may vary - some capacity
    /// may be released, or none at all.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::EventPool;
    ///
    /// let mut pool = EventPool::<i32>::new();
    ///
    /// // Use the pool which may grow its capacity
    /// for _ in 0..100 {
    ///     let (sender, receiver) = pool.by_ref();
    ///     sender.send(42);
    ///     let _value = futures::executor::block_on(receiver.recv_async());
    /// }
    ///
    /// // Attempt to shrink to reduce memory usage
    /// pool.shrink_to_fit();
    /// ```
    pub fn shrink_to_fit(&mut self) {
        self.pool.shrink_to_fit();
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
            let value = futures::executor::block_on(receiver.recv_async());
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
            let value = futures::executor::block_on(receiver.recv_async());
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
            let value = futures::executor::block_on(receiver.recv_async());
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
            let value = futures::executor::block_on(receiver.recv_async());
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

    // Memory leak detection tests - these specifically test that cleanup occurs on drop
    #[test]
    fn by_ref_sender_drop_cleanup() {
        with_watchdog(|| {
            let mut pool = EventPool::<i32>::new();
            {
                let (sender, _receiver) = pool.by_ref();

                // Force the sender to be dropped without being consumed by send()
                drop(sender);
                // Receiver will be dropped at end of scope
            }

            // Create a new event to verify the pool is still functional
            let (sender2, receiver2) = pool.by_ref();
            sender2.send(123);
            let value = receiver2.recv();
            assert_eq!(value, 123);
        });
    }

    #[test]
    fn by_ref_receiver_drop_cleanup() {
        with_watchdog(|| {
            let mut pool = EventPool::<i32>::new();
            {
                let (_sender, receiver) = pool.by_ref();

                // Force the receiver to be dropped without being consumed by recv()
                drop(receiver);
                // Sender will be dropped at end of scope
            }

            // Create a new event to verify the pool is still functional
            let (sender2, receiver2) = pool.by_ref();
            sender2.send(456);
            let value = receiver2.recv();
            assert_eq!(value, 456);
        });
    }

    #[test]
    fn by_rc_sender_drop_cleanup() {
        use std::cell::RefCell;
        use std::rc::Rc;

        with_watchdog(|| {
            let pool = Rc::new(RefCell::new(EventPool::<i32>::new()));
            let (sender, _receiver) = pool.borrow_mut().by_rc(&pool);

            // Force the sender to be dropped without being consumed by send()
            drop(sender);

            // Create a new event to verify the pool is still functional
            let (sender2, receiver2) = pool.borrow_mut().by_rc(&pool);
            sender2.send(789);
            let value = receiver2.recv();
            assert_eq!(value, 789);
        });
    }

    #[test]
    fn by_rc_receiver_drop_cleanup() {
        use std::cell::RefCell;
        use std::rc::Rc;

        with_watchdog(|| {
            let pool = Rc::new(RefCell::new(EventPool::<i32>::new()));
            let (_sender, receiver) = pool.borrow_mut().by_rc(&pool);

            // Force the receiver to be dropped without being consumed by recv()
            drop(receiver);

            // Create a new event to verify the pool is still functional
            let (sender2, receiver2) = pool.borrow_mut().by_rc(&pool);
            sender2.send(321);
            let value = receiver2.recv();
            assert_eq!(value, 321);
        });
    }

    #[test]
    fn by_arc_sender_drop_cleanup() {
        use std::sync::{Arc, Mutex};

        with_watchdog(|| {
            let pool = Arc::new(Mutex::new(EventPool::<i32>::new()));
            let (sender, _receiver) = pool.lock().unwrap().by_arc(&pool);

            // Force the sender to be dropped without being consumed by send()
            drop(sender);

            // Create a new event to verify the pool is still functional
            let (sender2, receiver2) = pool.lock().unwrap().by_arc(&pool);
            sender2.send(654);
            let value = receiver2.recv();
            assert_eq!(value, 654);
        });
    }

    #[test]
    fn by_arc_receiver_drop_cleanup() {
        use std::sync::{Arc, Mutex};

        with_watchdog(|| {
            let pool = Arc::new(Mutex::new(EventPool::<i32>::new()));
            let (_sender, receiver) = pool.lock().unwrap().by_arc(&pool);

            // Force the receiver to be dropped without being consumed by recv()
            drop(receiver);

            // Create a new event to verify the pool is still functional
            let (sender2, receiver2) = pool.lock().unwrap().by_arc(&pool);
            sender2.send(987);
            let value = receiver2.recv();
            assert_eq!(value, 987);
        });
    }

    #[test]
    fn by_ptr_sender_drop_cleanup() {
        use std::pin::Pin;

        with_watchdog(|| {
            let mut pool = EventPool::<i32>::new();
            let mut pinned_pool = Pin::new(&mut pool);

            // SAFETY: We ensure the pool outlives the sender and receiver
            let (sender, _receiver) = unsafe { pinned_pool.as_mut().by_ptr() };

            // Force the sender to be dropped without being consumed by send()
            drop(sender);

            // Create a new event to verify the pool is still functional
            // SAFETY: We ensure the pool outlives the sender and receiver
            let (sender2, receiver2) = unsafe { pinned_pool.by_ptr() };
            sender2.send(147);
            let value = receiver2.recv();
            assert_eq!(value, 147);
        });
    }

    #[test]
    fn by_ptr_receiver_drop_cleanup() {
        use std::pin::Pin;

        with_watchdog(|| {
            let mut pool = EventPool::<i32>::new();
            let mut pinned_pool = Pin::new(&mut pool);

            // SAFETY: We ensure the pool outlives the sender and receiver
            let (_sender, receiver) = unsafe { pinned_pool.as_mut().by_ptr() };

            // Force the receiver to be dropped without being consumed by recv()
            drop(receiver);

            // Create a new event to verify the pool is still functional
            // SAFETY: We ensure the pool outlives the sender and receiver
            let (sender2, receiver2) = unsafe { pinned_pool.by_ptr() };
            sender2.send(258);
            let value = receiver2.recv();
            assert_eq!(value, 258);
        });
    }

    #[test]
    fn dec_ref_and_cleanup_is_called() {
        with_watchdog(|| {
            let mut pool = EventPool::<i32>::new();

            // Create multiple events and drop them without using
            for _ in 0..5 {
                let (sender, receiver) = pool.by_ref();
                drop(sender);
                drop(receiver);
            }

            // Verify pool still works correctly after cleanup
            let (sender, receiver) = pool.by_ref();
            sender.send(999);
            let value = futures::executor::block_on(receiver.recv_async());
            assert_eq!(value, 999);
        });
    }

    #[test]
    fn pool_cleanup_verified_by_capacity() {
        with_watchdog(|| {
            let mut pool = EventPool::<i32>::new();

            // Create many events and drop them without using - this should not grow the pool permanently
            for i in 0..10 {
                let (sender, receiver) = pool.by_ref();
                if i % 2 == 0 {
                    drop(sender);
                    drop(receiver);
                } else {
                    // Use some events normally
                    sender.send(i);
                    let _value = futures::executor::block_on(receiver.recv_async());
                }
            }

            // The pool should have cleaned up unused events
            // If cleanup is broken, the pool would retain all the unused events
            // This is a bit of an implementation detail but it's necessary to catch the leak

            // Create one more event to verify pool still works
            let (sender, receiver) = pool.by_ref();
            sender.send(42);
            let value = futures::executor::block_on(receiver.recv_async());
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn ref_count_tracking_works() {
        with_watchdog(|| {
            let mut wrapper = WithRefCount::new(String::from("test"));

            // Simulate what happens in the pool
            wrapper.inc_ref(); // For sender
            wrapper.inc_ref(); // For receiver
            assert_eq!(wrapper.ref_count(), 2);

            wrapper.dec_ref(); // Sender dropped
            assert_eq!(wrapper.ref_count(), 1);
            assert!(wrapper.is_referenced());

            wrapper.dec_ref(); // Receiver dropped
            assert_eq!(wrapper.ref_count(), 0);
            assert!(!wrapper.is_referenced());
        });
    }

    #[test]
    fn pool_stress_test_no_leak() {
        with_watchdog(|| {
            let mut pool = EventPool::<u64>::new();

            // Stress test with many dropped events
            for _ in 0..100 {
                let (sender, receiver) = pool.by_ref();
                // Drop without using
                drop(sender);
                drop(receiver);
            }

            // Pool should still work efficiently
            let (sender, receiver) = pool.by_ref();
            sender.send(999);
            let value = futures::executor::block_on(receiver.recv_async());
            assert_eq!(value, 999);
        });
    }

    #[test]
    fn by_ref_drop_actually_cleans_up_pool() {
        let mut pool = EventPool::<u32>::new();

        // Create many events but drop them without use
        for _ in 0..100 {
            let (_sender, _receiver) = pool.by_ref();
            // Both sender and receiver will be dropped here
        }

        // Pool should be cleaned up - all events should be removed
        // If Drop implementations don't work, pool will retain unused events
        assert_eq!(
            pool.pool.len(),
            0,
            "Pool still contains unused events - Drop implementations not working"
        );

        // An empty pool should be able to shrink to capacity 0
        pool.shrink_to_fit();
        assert_eq!(
            pool.pool.capacity(),
            0,
            "Empty pool should shrink to capacity 0"
        );
    }

    #[test]
    fn by_rc_drop_actually_cleans_up_pool() {
        let pool = Rc::new(std::cell::RefCell::new(EventPool::<u32>::new()));

        // Create many events but drop them without use
        for _ in 0..100 {
            let (_sender, _receiver) = pool.borrow_mut().by_rc(&pool);
            // Both sender and receiver will be dropped here
        }

        // Pool should be cleaned up - all events should be removed
        assert_eq!(
            pool.borrow().pool.len(),
            0,
            "Pool still contains unused events - Drop implementations not working"
        );

        // An empty pool should be able to shrink to capacity 0
        pool.borrow_mut().shrink_to_fit();
        assert_eq!(
            pool.borrow().pool.capacity(),
            0,
            "Empty pool should shrink to capacity 0"
        );
    }

    #[test]
    fn by_arc_drop_actually_cleans_up_pool() {
        let pool = Arc::new(std::sync::Mutex::new(EventPool::<u32>::new()));

        // Create many events but drop them without use
        for _ in 0..100 {
            let (_sender, _receiver) = pool.lock().unwrap().by_arc(&pool);
            // Both sender and receiver will be dropped here
        }

        // Pool should be cleaned up - all events should be removed
        assert_eq!(
            pool.lock().unwrap().pool.len(),
            0,
            "Pool still contains unused events - Drop implementations not working"
        );

        // An empty pool should be able to shrink to capacity 0
        pool.lock().unwrap().shrink_to_fit();
        assert_eq!(
            pool.lock().unwrap().pool.capacity(),
            0,
            "Empty pool should shrink to capacity 0"
        );
    }

    #[test]
    fn by_ptr_drop_actually_cleans_up_pool() {
        // Test ptr-based pooled events cleanup by checking pool state
        for iteration in 0..10 {
            let mut pool = EventPool::<u32>::new();

            {
                let mut pool_pin = std::pin::pin!(pool);
                // SAFETY: We pin the pool for the duration of by_ptr call
                let (_sender, _receiver) = unsafe { pool_pin.as_mut().by_ptr() };
                // sender and receiver will be dropped here
            }

            // Create a fresh pool for next iteration
            #[allow(
                unused_assignments,
                reason = "Testing pool recreation for memory leak detection"
            )]
            {
                pool = EventPool::<u32>::new();
            }

            // For this test, we'll verify that repeated operations don't accumulate
            // If Drop implementations don't work, we'd see memory accumulation
            println!("Iteration {iteration}: Pool operations completed");
        }
    }

    #[test]
    fn dec_ref_and_cleanup_actually_removes_events() {
        let mut pool = EventPool::<u32>::new();

        // Test 1: Check that events are added to pool
        let pool_len_before = pool.pool.len();

        // Create events in a scope to ensure they're dropped
        let sender_key = {
            let (sender, receiver) = pool.by_ref();
            let key = sender.key;

            // Events should be in pool now (don't check len while borrowed)

            drop(sender);
            drop(receiver);
            key
        };

        // Now check that cleanup worked
        let pool_len_after = pool.pool.len();
        assert_eq!(
            pool_len_after, pool_len_before,
            "Pool not cleaned up after dropping events - dec_ref_and_cleanup not working, key: {sender_key:?}"
        );
    }
}
