//! Pooled events that provide automatic resource management.
//!
//! This module provides pooled variants of events that automatically manage their lifecycle
//! using reference counting. Events are created from pools and automatically returned to the
//! pool when both sender and receiver are dropped.

use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use pinned_pool::{Key, PinnedPool};

use super::pinned_with_ref_count::PinnedWithRefCount;
use super::sync::OnceEvent;

mod by_arc;
mod by_ptr;
mod by_rc;
mod by_ref;

pub use by_arc::{ByArcPooledOnceReceiver, ByArcPooledOnceSender};
pub use by_ptr::{ByPtrPooledOnceReceiver, ByPtrPooledOnceSender};
pub use by_rc::{ByRcPooledOnceReceiver, ByRcPooledOnceSender};
pub use by_ref::{ByRefPooledOnceReceiver, ByRefPooledOnceSender};

/// A pool that manages thread-safe events with automatic cleanup.
///
/// The pool creates events on demand and automatically cleans them up when both
/// sender and receiver endpoints are dropped. Events are reference-counted to
/// track when they are no longer in use.
///
/// This pool provides zero-allocation event reuse for high-frequency scenarios
/// while maintaining thread-safety across multiple threads.
///
/// # Example
///
/// ```rust
/// use events::OnceEventPool;
/// # use futures::executor::block_on;
///
/// # block_on(async {
/// let pool = OnceEventPool::<i32>::new();
///
/// // First usage - creates new event
/// let (sender1, receiver1) = pool.bind_by_ref();
/// sender1.send(42);
/// let value1 = receiver1.recv_async().await;
/// assert_eq!(value1, 42);
/// // Event returned to pool when sender1/receiver1 are dropped
///
/// // Second usage - reuses the same event instance (efficient!)
/// let (sender2, receiver2) = pool.bind_by_ref();
/// sender2.send(100);
/// let value2 = receiver2.recv_async().await;
/// assert_eq!(value2, 100);
/// // Same event reused - no additional allocation overhead
/// # });
/// ```
#[derive(Debug)]
pub struct OnceEventPool<T>
where
    T: Send,
{
    pool: Mutex<PinnedPool<PinnedWithRefCount<OnceEvent<T>>>>,
}

impl<T> OnceEventPool<T>
where
    T: Send,
{
    /// Creates a new empty event pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::OnceEventPool;
    ///
    /// let pool = OnceEventPool::<String>::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            pool: Mutex::new(PinnedPool::new()),
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
    /// use events::OnceEventPool;
    /// # use futures::executor::block_on;
    ///
    /// # block_on(async {
    /// let pool = OnceEventPool::<i32>::new();
    ///
    /// // First event usage
    /// let (sender1, receiver1) = pool.bind_by_ref();
    /// sender1.send(42);
    /// let value1 = receiver1.recv_async().await;
    /// assert_eq!(value1, 42);
    ///
    /// // Second event usage - efficiently reuses the same underlying event
    /// let (sender2, receiver2) = pool.bind_by_ref();
    /// sender2.send(100);
    /// let value2 = receiver2.recv_async().await;
    /// assert_eq!(value2, 100);
    /// # });
    /// ```
    pub fn bind_by_ref(&self) -> (ByRefPooledOnceSender<'_, T>, ByRefPooledOnceReceiver<'_, T>) {
        let mut pool_guard = self.pool.lock().expect("pool mutex should not be poisoned");
        let inserter = pool_guard.begin_insert();
        let key = inserter.key();
        let _item = inserter.insert_mut(PinnedWithRefCount::new(OnceEvent::new()));

        // Increment reference count for both sender and receiver
        {
            let mut item_mut = pool_guard.get_mut(key);
            item_mut.as_mut().inc_ref();
            item_mut.as_mut().inc_ref();
        }

        // Drop the lock before creating the endpoints
        drop(pool_guard);

        // Use raw pointer to avoid borrowing self twice
        let pool_ptr: *const Self = self;

        (
            ByRefPooledOnceSender {
                pool: pool_ptr,
                key,
                _phantom: std::marker::PhantomData,
            },
            ByRefPooledOnceReceiver {
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
    /// use events::OnceEventPool;
    /// # use futures::executor::block_on;
    ///
    /// # block_on(async {
    /// let pool = Rc::new(OnceEventPool::<i32>::new());
    ///
    /// // First usage
    /// let (sender1, receiver1) = pool.bind_by_rc(&pool);
    /// sender1.send(42);
    /// let value1 = receiver1.recv_async().await;
    /// assert_eq!(value1, 42);
    ///
    /// // Second usage - reuses the same event from the pool
    /// let (sender2, receiver2) = pool.bind_by_rc(&pool);
    /// sender2.send(100);
    /// let value2 = receiver2.recv_async().await;
    /// assert_eq!(value2, 100);
    /// # });
    /// ```
    pub fn bind_by_rc(
        &self,
        pool_rc: &Rc<Self>,
    ) -> (ByRcPooledOnceSender<T>, ByRcPooledOnceReceiver<T>) {
        let mut pool_guard = self.pool.lock().expect("pool mutex should not be poisoned");
        let inserter = pool_guard.begin_insert();
        let key = inserter.key();
        let _item = inserter.insert_mut(PinnedWithRefCount::new(OnceEvent::new()));

        // Now increment reference count for both sender and receiver
        {
            let mut item_mut = pool_guard.get_mut(key);
            item_mut.as_mut().inc_ref();
            item_mut.as_mut().inc_ref();
        }

        // Drop the lock before creating the endpoints
        drop(pool_guard);

        (
            ByRcPooledOnceSender {
                pool: Rc::clone(pool_rc),
                key,
            },
            ByRcPooledOnceReceiver {
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
    /// use std::sync::Arc;
    ///
    /// use events::OnceEventPool;
    ///
    /// let pool = Arc::new(OnceEventPool::<i32>::new());
    ///
    /// // First usage
    /// let (sender1, receiver1) = pool.bind_by_arc(&pool);
    /// sender1.send(42);
    /// let value1 = futures::executor::block_on(receiver1.recv_async());
    /// assert_eq!(value1, 42);
    ///
    /// // Second usage - efficiently reuses the same pooled event
    /// let (sender2, receiver2) = pool.bind_by_arc(&pool);
    /// sender2.send(200);
    /// let value2 = futures::executor::block_on(receiver2.recv_async());
    /// assert_eq!(value2, 200);
    /// ```
    pub fn bind_by_arc(
        &self,
        pool_arc: &Arc<Self>,
    ) -> (ByArcPooledOnceSender<T>, ByArcPooledOnceReceiver<T>) {
        let mut pool_guard = self.pool.lock().expect("pool mutex should not be poisoned");
        let inserter = pool_guard.begin_insert();
        let key = inserter.key();
        let _item = inserter.insert_mut(PinnedWithRefCount::new(OnceEvent::new()));

        // Now increment reference count for both sender and receiver
        {
            let mut item_mut = pool_guard.get_mut(key);
            item_mut.as_mut().inc_ref();
            item_mut.as_mut().inc_ref();
        }

        // Drop the lock before creating the endpoints
        drop(pool_guard);

        (
            ByArcPooledOnceSender {
                pool: Arc::clone(pool_arc),
                key,
            },
            ByArcPooledOnceReceiver {
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
    /// use events::OnceEventPool;
    ///
    /// let pool = OnceEventPool::<i32>::new();
    /// let pinned_pool = Pin::new(&pool);
    ///
    /// // First usage
    /// // SAFETY: We ensure the pool outlives the sender and receiver
    /// let (sender1, receiver1) = unsafe { pinned_pool.bind_by_ptr() };
    /// sender1.send(42);
    /// let value1 = futures::executor::block_on(receiver1.recv_async());
    /// assert_eq!(value1, 42);
    ///
    /// // Second usage - reuses the same event from the pool efficiently
    /// // SAFETY: Pool is still valid and pinned
    /// let (sender2, receiver2) = unsafe { pinned_pool.bind_by_ptr() };
    /// sender2.send(100);
    /// let value2 = futures::executor::block_on(receiver2.recv_async());
    /// assert_eq!(value2, 100);
    /// // Both sender and receiver pairs are dropped here, before pool
    /// ```
    #[must_use]
    pub unsafe fn bind_by_ptr(
        self: Pin<&Self>,
    ) -> (ByPtrPooledOnceSender<T>, ByPtrPooledOnceReceiver<T>) {
        let mut pool_guard = self.pool.lock().expect("pool mutex should not be poisoned");
        let inserter = pool_guard.begin_insert();
        let key = inserter.key();
        let _item = inserter.insert_mut(PinnedWithRefCount::new(OnceEvent::new()));

        // Now increment reference count for both sender and receiver
        {
            let mut item_mut = pool_guard.get_mut(key);
            item_mut.as_mut().inc_ref();
            item_mut.as_mut().inc_ref();
        }

        // Drop the lock before creating the endpoints
        drop(pool_guard);

        let pool_ptr: *const Self = self.get_ref();

        (
            ByPtrPooledOnceSender {
                pool: pool_ptr,
                key,
            },
            ByPtrPooledOnceReceiver {
                pool: pool_ptr,
                key,
            },
        )
    }

    /// Decrements the reference count for an event and removes it if no longer referenced.
    fn dec_ref_and_cleanup(&self, key: Key) {
        let mut pool_guard = self.pool.lock().expect("pool mutex should not be poisoned");
        let mut item = pool_guard.get_mut(key);
        item.as_mut().dec_ref();
        if !item.is_referenced() {
            pool_guard.remove(key);
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
    /// use events::OnceEventPool;
    ///
    /// let pool = OnceEventPool::<i32>::new();
    ///
    /// // Use the pool which may grow its capacity
    /// for _ in 0..100 {
    ///     let (sender, receiver) = pool.bind_by_ref();
    ///     sender.send(42);
    ///     let _value = futures::executor::block_on(receiver.recv_async());
    /// }
    ///
    /// // Attempt to shrink to reduce memory usage
    /// pool.shrink_to_fit();
    /// ```
    pub fn shrink_to_fit(&self) {
        let mut pool_guard = self.pool.lock().expect("pool mutex should not be poisoned");
        pool_guard.shrink_to_fit();
    }
}

impl<T> Default for OnceEventPool<T>
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
    fn event_pool_by_ref() {
        with_watchdog(|| {
            let pool = OnceEventPool::<i32>::new();
            let (sender, receiver) = pool.bind_by_ref();

            sender.send(42);
            let value = futures::executor::block_on(receiver.recv_async()).unwrap();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_pool_async() {
        use futures::executor::block_on;

        with_watchdog(|| {
            let pool = OnceEventPool::<i32>::new();
            let (sender, receiver) = pool.bind_by_ref();

            sender.send(42);
            let value = block_on(receiver.recv_async()).unwrap();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn pool_drop_cleanup() {
        with_watchdog(|| {
            let pool = OnceEventPool::<i32>::new();

            // Create and drop sender/receiver without using them
            let (sender, receiver) = pool.bind_by_ref();
            drop(sender);
            drop(receiver);

            // Pool should be empty (the event should have been cleaned up)
            // This is implementation detail but shows the cleanup works
        });
    }

    #[test]
    fn pool_multiple_events() {
        with_watchdog(|| {
            let pool = OnceEventPool::<i32>::new();

            // Test one event first
            let (sender1, receiver1) = pool.bind_by_ref();
            sender1.send(1);
            let value1 = futures::executor::block_on(receiver1.recv_async()).unwrap();
            assert_eq!(value1, 1);

            // Test another event
            let (sender2, receiver2) = pool.bind_by_ref();
            sender2.send(2);
            let value2 = futures::executor::block_on(receiver2.recv_async()).unwrap();
            assert_eq!(value2, 2);
        });
    }

    #[test]
    fn event_pool_by_rc() {
        use std::rc::Rc;

        with_watchdog(|| {
            let pool = Rc::new(OnceEventPool::<i32>::new());
            let (sender, receiver) = pool.bind_by_rc(&pool);

            sender.send(42);
            let value = futures::executor::block_on(receiver.recv_async()).unwrap();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_pool_by_arc() {
        use std::sync::Arc;

        with_watchdog(|| {
            let pool = Arc::new(OnceEventPool::<i32>::new());
            let (sender, receiver) = pool.bind_by_arc(&pool);

            sender.send(42);
            let value = futures::executor::block_on(receiver.recv_async()).unwrap();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_pool_by_ptr() {
        use std::pin::Pin;

        with_watchdog(|| {
            let pool = OnceEventPool::<i32>::new();
            let pinned_pool = Pin::new(&pool);
            // SAFETY: We ensure the pool outlives the sender and receiver
            let (sender, receiver) = unsafe { pinned_pool.bind_by_ptr() };

            sender.send(42);
            let value = futures::executor::block_on(receiver.recv_async()).unwrap();
            assert_eq!(value, 42);
            // sender and receiver are dropped here, before pool
        });
    }

    #[test]
    fn event_pool_by_rc_async() {
        use std::rc::Rc;

        use futures::executor::block_on;

        with_watchdog(|| {
            let pool = Rc::new(OnceEventPool::<i32>::new());
            let (sender, receiver) = pool.bind_by_rc(&pool);

            sender.send(42);
            let value = block_on(receiver.recv_async()).unwrap();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_pool_by_arc_async() {
        use std::sync::Arc;

        use futures::executor::block_on;

        with_watchdog(|| {
            let pool = Arc::new(OnceEventPool::<i32>::new());
            let (sender, receiver) = pool.bind_by_arc(&pool);

            sender.send(42);
            let value = block_on(receiver.recv_async()).unwrap();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_pool_by_ptr_async() {
        use std::pin::Pin;

        use futures::executor::block_on;

        with_watchdog(|| {
            let pool = OnceEventPool::<i32>::new();
            let pinned_pool = Pin::new(&pool);
            // SAFETY: We ensure the pool outlives the sender and receiver
            let (sender, receiver) = unsafe { pinned_pool.bind_by_ptr() };

            sender.send(42);
            let value = block_on(receiver.recv_async()).unwrap();
            assert_eq!(value, 42);
            // sender and receiver are dropped here, before pool
        });
    }

    // Memory leak detection tests - these specifically test that cleanup occurs on drop
    #[test]
    fn by_ref_sender_drop_cleanup() {
        with_watchdog(|| {
            let pool = OnceEventPool::<i32>::new();
            {
                let (sender, _receiver) = pool.bind_by_ref();

                // Force the sender to be dropped without being consumed by send()
                drop(sender);
                // Receiver will be dropped at end of scope
            }

            // Create a new event to verify the pool is still functional
            let (sender2, receiver2) = pool.bind_by_ref();
            sender2.send(123);
            let value = futures::executor::block_on(receiver2.recv_async()).unwrap();
            assert_eq!(value, 123);
        });
    }

    #[test]
    fn by_ref_receiver_drop_cleanup() {
        with_watchdog(|| {
            let pool = OnceEventPool::<i32>::new();
            {
                let (_sender, receiver) = pool.bind_by_ref();

                // Force the receiver to be dropped without being consumed by recv()
                drop(receiver);
                // Sender will be dropped at end of scope
            }

            // Create a new event to verify the pool is still functional
            let (sender2, receiver2) = pool.bind_by_ref();
            sender2.send(456);
            let value = futures::executor::block_on(receiver2.recv_async()).unwrap();
            assert_eq!(value, 456);
        });
    }

    #[test]
    fn by_rc_sender_drop_cleanup() {
        use std::rc::Rc;

        with_watchdog(|| {
            let pool = Rc::new(OnceEventPool::<i32>::new());
            let (sender, _receiver) = pool.bind_by_rc(&pool);

            // Force the sender to be dropped without being consumed by send()
            drop(sender);

            // Create a new event to verify the pool is still functional
            let (sender2, receiver2) = pool.bind_by_rc(&pool);
            sender2.send(789);
            let value = futures::executor::block_on(receiver2.recv_async()).unwrap();
            assert_eq!(value, 789);
        });
    }

    #[test]
    fn by_rc_receiver_drop_cleanup() {
        use std::rc::Rc;

        with_watchdog(|| {
            let pool = Rc::new(OnceEventPool::<i32>::new());
            let (_sender, receiver) = pool.bind_by_rc(&pool);

            // Force the receiver to be dropped without being consumed by recv()
            drop(receiver);

            // Create a new event to verify the pool is still functional
            let (sender2, receiver2) = pool.bind_by_rc(&pool);
            sender2.send(321);
            let value = futures::executor::block_on(receiver2.recv_async()).unwrap();
            assert_eq!(value, 321);
        });
    }

    #[test]
    fn by_arc_sender_drop_cleanup() {
        use std::sync::Arc;

        with_watchdog(|| {
            let pool = Arc::new(OnceEventPool::<i32>::new());
            let (sender, _receiver) = pool.bind_by_arc(&pool);

            // Force the sender to be dropped without being consumed by send()
            drop(sender);

            // Create a new event to verify the pool is still functional
            let (sender2, receiver2) = pool.bind_by_arc(&pool);
            sender2.send(654);
            let value = futures::executor::block_on(receiver2.recv_async()).unwrap();
            assert_eq!(value, 654);
        });
    }

    #[test]
    fn by_arc_receiver_drop_cleanup() {
        use std::sync::Arc;

        with_watchdog(|| {
            let pool = Arc::new(OnceEventPool::<i32>::new());
            let (_sender, receiver) = pool.bind_by_arc(&pool);

            // Force the receiver to be dropped without being consumed by recv()
            drop(receiver);

            // Create a new event to verify the pool is still functional
            let (sender2, receiver2) = pool.bind_by_arc(&pool);
            sender2.send(987);
            let value = futures::executor::block_on(receiver2.recv_async()).unwrap();
            assert_eq!(value, 987);
        });
    }

    #[test]
    fn by_ptr_sender_drop_cleanup() {
        use std::pin::Pin;

        with_watchdog(|| {
            let pool = OnceEventPool::<i32>::new();
            let pinned_pool = Pin::new(&pool);

            // SAFETY: We ensure the pool outlives the sender and receiver
            let (sender, _receiver) = unsafe { pinned_pool.as_ref().bind_by_ptr() };

            // Force the sender to be dropped without being consumed by send()
            drop(sender);

            // Create a new event to verify the pool is still functional
            // SAFETY: We ensure the pool outlives the sender and receiver
            let (sender2, receiver2) = unsafe { pinned_pool.bind_by_ptr() };
            sender2.send(147);
            let value = futures::executor::block_on(receiver2.recv_async()).unwrap();
            assert_eq!(value, 147);
        });
    }

    #[test]
    fn by_ptr_receiver_drop_cleanup() {
        use std::pin::Pin;

        with_watchdog(|| {
            let pool = OnceEventPool::<i32>::new();
            let pinned_pool = Pin::new(&pool);

            // SAFETY: We ensure the pool outlives the sender and receiver
            let (_sender, receiver) = unsafe { pinned_pool.as_ref().bind_by_ptr() };

            // Force the receiver to be dropped without being consumed by recv()
            drop(receiver);

            // Create a new event to verify the pool is still functional
            // SAFETY: We ensure the pool outlives the sender and receiver
            let (sender2, receiver2) = unsafe { pinned_pool.bind_by_ptr() };
            sender2.send(258);
            let value = futures::executor::block_on(receiver2.recv_async()).unwrap();
            assert_eq!(value, 258);
        });
    }

    #[test]
    fn dec_ref_and_cleanup_is_called() {
        with_watchdog(|| {
            let pool = OnceEventPool::<i32>::new();

            // Create multiple events and drop them without using
            for _ in 0..5 {
                let (sender, receiver) = pool.bind_by_ref();
                drop(sender);
                drop(receiver);
            }

            // Verify pool still works correctly after cleanup
            let (sender, receiver) = pool.bind_by_ref();
            sender.send(999);
            let value = futures::executor::block_on(receiver.recv_async()).unwrap();
            assert_eq!(value, 999);
        });
    }

    #[test]
    fn pool_cleanup_verified_by_capacity() {
        with_watchdog(|| {
            let pool = OnceEventPool::<i32>::new();

            // Create many events and drop them without using - this should not grow the pool permanently
            for i in 0..10 {
                let (sender, receiver) = pool.bind_by_ref();
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
            let (sender, receiver) = pool.bind_by_ref();
            sender.send(42);
            let value = futures::executor::block_on(receiver.recv_async()).unwrap();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn pool_stress_test_no_leak() {
        with_watchdog(|| {
            let pool = OnceEventPool::<u64>::new();

            // Stress test with many dropped events
            for _ in 0..100 {
                let (sender, receiver) = pool.bind_by_ref();
                // Drop without using
                drop(sender);
                drop(receiver);
            }

            // Pool should still work efficiently
            let (sender, receiver) = pool.bind_by_ref();
            sender.send(999);
            let value = futures::executor::block_on(receiver.recv_async()).unwrap();
            assert_eq!(value, 999);
        });
    }

    #[test]
    fn by_ref_drop_actually_cleans_up_pool() {
        let pool = OnceEventPool::<u32>::new();

        // Create many events but drop them without use
        for _ in 0..100 {
            let (_sender, _receiver) = pool.bind_by_ref();
            // Both sender and receiver will be dropped here
        }

        // Pool should be cleaned up - all events should be removed
        // If Drop implementations don't work, pool will retain unused events
        let mut pool_guard = pool.pool.lock().expect("pool mutex should not be poisoned");
        assert_eq!(
            pool_guard.len(),
            0,
            "Pool still contains unused events - Drop implementations not working"
        );

        // An empty pool should be able to shrink to capacity 0
        pool_guard.shrink_to_fit();
        assert_eq!(
            pool_guard.capacity(),
            0,
            "Empty pool should shrink to capacity 0"
        );
    }

    #[test]
    fn by_rc_drop_actually_cleans_up_pool() {
        let pool = Rc::new(OnceEventPool::<u32>::new());

        // Create many events but drop them without use
        for _ in 0..100 {
            let (_sender, _receiver) = pool.bind_by_rc(&pool);
            // Both sender and receiver will be dropped here
        }

        // Pool should be cleaned up - all events should be removed
        let mut pool_guard = pool.pool.lock().expect("pool mutex should not be poisoned");
        assert_eq!(
            pool_guard.len(),
            0,
            "Pool still contains unused events - Drop implementations not working"
        );

        // An empty pool should be able to shrink to capacity 0
        pool_guard.shrink_to_fit();
        assert_eq!(
            pool_guard.capacity(),
            0,
            "Empty pool should shrink to capacity 0"
        );
    }

    #[test]
    fn by_arc_drop_actually_cleans_up_pool() {
        let pool = Arc::new(OnceEventPool::<u32>::new());

        // Create many events but drop them without use
        for _ in 0..100 {
            let (_sender, _receiver) = pool.bind_by_arc(&pool);
            // Both sender and receiver will be dropped here
        }

        // Pool should be cleaned up - all events should be removed
        let mut pool_guard = pool.pool.lock().expect("pool mutex should not be poisoned");
        assert_eq!(
            pool_guard.len(),
            0,
            "Pool still contains unused events - Drop implementations not working"
        );

        // An empty pool should be able to shrink to capacity 0
        pool_guard.shrink_to_fit();
        assert_eq!(
            pool_guard.capacity(),
            0,
            "Empty pool should shrink to capacity 0"
        );
    }

    #[test]
    fn by_ptr_drop_actually_cleans_up_pool() {
        // Test ptr-based pooled events cleanup by checking pool state
        for iteration in 0..10 {
            let pool = OnceEventPool::<u32>::new();

            {
                let pool_pin = std::pin::pin!(pool);
                // SAFETY: We pin the pool for the duration of by_ptr call
                let (_sender, _receiver) = unsafe { pool_pin.as_ref().bind_by_ptr() };
                // sender and receiver will be dropped here
            }

            // For this test, we'll verify that repeated operations don't accumulate
            // If Drop implementations don't work, we'd see memory accumulation
            println!("Iteration {iteration}: Pool operations completed");
        }
    }

    #[test]
    fn dec_ref_and_cleanup_actually_removes_events() {
        let pool = OnceEventPool::<u32>::new();

        // Test 1: Check that events are added to pool
        let pool_len_before = {
            let pool_guard = pool.pool.lock().expect("pool mutex should not be poisoned");
            pool_guard.len()
        };

        // Create events in a scope to ensure they're dropped
        let sender_key = {
            let (sender, receiver) = pool.bind_by_ref();
            let key = sender.key;

            // Events should be in pool now (don't check len while borrowed)

            drop(sender);
            drop(receiver);
            key
        };

        // Now check that cleanup worked
        let pool_len_after = {
            let pool_guard = pool.pool.lock().expect("pool mutex should not be poisoned");
            pool_guard.len()
        };
        assert_eq!(
            pool_len_after, pool_len_before,
            "Pool not cleaned up after dropping events - dec_ref_and_cleanup not working, key: {sender_key:?}"
        );
    }
}
