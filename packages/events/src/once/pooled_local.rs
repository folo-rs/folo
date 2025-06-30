//! Pooled local events that provide automatic resource management for single-threaded use.
//!
//! This module provides pooled variants of local events that automatically manage their lifecycle
//! using reference counting. Events are created from pools and automatically returned to the
//! pool when both sender and receiver are dropped.

use std::cell::RefCell;
use std::pin::Pin;
use std::rc::Rc;

use pinned_pool::{Key, PinnedPool};

use super::local::LocalOnceEvent;
use super::pinned_with_ref_count::PinnedWithRefCount;

mod by_ptr;
mod by_rc;
mod by_ref;

pub use by_ptr::{ByPtrPooledLocalOnceReceiver, ByPtrPooledLocalOnceSender};
pub use by_rc::{ByRcPooledLocalOnceReceiver, ByRcPooledLocalOnceSender};
pub use by_ref::{ByRefPooledLocalOnceReceiver, ByRefPooledLocalOnceSender};

/// A pool that manages single-threaded events with automatic cleanup.
///
/// The pool creates local events on demand and automatically cleans them up when both
/// sender and receiver endpoints are dropped. Events are reference-counted to
/// track when they are no longer in use.
///
/// This is the single-threaded variant that cannot be shared across threads but has
/// lower overhead than the thread-safe [`super::OnceEventPool`].
///
/// The pool provides zero-allocation event reuse for high-frequency scenarios
/// within a single thread.
///
/// # Example
///
/// ```rust
/// use events::LocalOnceEventPool;
///
/// let pool = LocalOnceEventPool::<i32>::new();
///
/// // First usage - creates new event
/// let (sender1, receiver1) = pool.bind_by_ref();
/// sender1.send(42);
/// let value1 = futures::executor::block_on(receiver1).unwrap();
/// assert_eq!(value1, 42);
/// // Event returned to pool when sender1/receiver1 are dropped
///
/// // Second usage - reuses the same event instance for efficiency
/// let (sender2, receiver2) = pool.bind_by_ref();
/// sender2.send(100);
/// let value2 = futures::executor::block_on(receiver2).unwrap();
/// assert_eq!(value2, 100);
/// // Same event reused - no additional allocation overhead
/// ```
#[derive(Debug)]
pub struct LocalOnceEventPool<T> {
    pool: RefCell<PinnedPool<PinnedWithRefCount<LocalOnceEvent<T>>>>,
}

impl<T> LocalOnceEventPool<T> {
    /// Creates a new empty local event pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::LocalOnceEventPool;
    ///
    /// let pool = LocalOnceEventPool::<String>::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            pool: RefCell::new(PinnedPool::new()),
        }
    }

    /// Creates sender and receiver endpoints connected by reference to the pool.
    ///
    /// The pool will create a new local event and return endpoints that reference it.
    /// When both endpoints are dropped, the event will be automatically cleaned up.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::LocalOnceEventPool;
    ///
    /// let pool = LocalOnceEventPool::<i32>::new();
    ///
    /// // First usage
    /// let (sender1, receiver1) = pool.bind_by_ref();
    /// sender1.send(42);
    /// let value1 = futures::executor::block_on(receiver1).unwrap();
    /// assert_eq!(value1, 42);
    ///
    /// // Second usage - efficiently reuses the same underlying event
    /// let (sender2, receiver2) = pool.bind_by_ref();
    /// sender2.send(100);
    /// let value2 = futures::executor::block_on(receiver2).unwrap();
    /// assert_eq!(value2, 100);
    /// ```
    pub fn bind_by_ref(
        &self,
    ) -> (
        ByRefPooledLocalOnceSender<'_, T>,
        ByRefPooledLocalOnceReceiver<'_, T>,
    ) {
        let mut pool_borrow = self.pool.borrow_mut();
        let inserter = pool_borrow.begin_insert();
        let key = inserter.key();
        let _item = inserter.insert_mut(PinnedWithRefCount::new(LocalOnceEvent::new()));

        // Increment reference count for both sender and receiver
        {
            let mut item_mut = pool_borrow.get_mut(key);
            item_mut.as_mut().inc_ref();
            item_mut.as_mut().inc_ref();
        }

        // Drop the borrow before creating the endpoints
        drop(pool_borrow);

        // Use raw pointer to avoid borrowing self twice
        let pool_ptr: *const Self = self;

        (
            ByRefPooledLocalOnceSender {
                pool: pool_ptr,
                key,
                _phantom: std::marker::PhantomData,
            },
            ByRefPooledLocalOnceReceiver {
                pool: pool_ptr,
                key: Some(key),
                _phantom: std::marker::PhantomData,
            },
        )
    }

    /// Creates sender and receiver endpoints connected by Rc to the pool.
    ///
    /// The pool will create a new local event and return endpoints that hold Rc references.
    /// When both endpoints are dropped, the event will be automatically cleaned up.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::rc::Rc;
    ///
    /// use events::LocalOnceEventPool;
    ///
    /// let pool = Rc::new(LocalOnceEventPool::<i32>::new());
    ///
    /// // First usage
    /// let (sender1, receiver1) = pool.bind_by_rc(&pool);
    /// sender1.send(42);
    /// let value1 = futures::executor::block_on(receiver1).unwrap();
    /// assert_eq!(value1, 42);
    ///
    /// // Second usage - reuses the same event from the pool
    /// let (sender2, receiver2) = pool.bind_by_rc(&pool);
    /// sender2.send(200);
    /// let value2 = futures::executor::block_on(receiver2).unwrap();
    /// assert_eq!(value2, 200);
    /// ```
    pub fn bind_by_rc(
        &self,
        pool_rc: &Rc<Self>,
    ) -> (ByRcPooledLocalOnceSender<T>, ByRcPooledLocalOnceReceiver<T>) {
        let mut pool_borrow = self.pool.borrow_mut();
        let inserter = pool_borrow.begin_insert();
        let key = inserter.key();
        let _item = inserter.insert_mut(PinnedWithRefCount::new(LocalOnceEvent::new()));

        // Now increment reference count for both sender and receiver
        {
            let mut item_mut = pool_borrow.get_mut(key);
            item_mut.as_mut().inc_ref();
            item_mut.as_mut().inc_ref();
        }

        // Drop the borrow before creating the endpoints
        drop(pool_borrow);

        (
            ByRcPooledLocalOnceSender {
                pool: Rc::clone(pool_rc),
                key,
            },
            ByRcPooledLocalOnceReceiver {
                pool: Rc::clone(pool_rc),
                key: Some(key),
            },
        )
    }

    /// Creates sender and receiver endpoints connected by raw pointer to the pool.
    ///
    /// The pool will create a new local event and return endpoints that hold raw pointers.
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
    /// use events::LocalOnceEventPool;
    /// # use futures::executor::block_on;
    ///
    /// # block_on(async {
    /// let pool = LocalOnceEventPool::<i32>::new();
    /// let pinned_pool = Pin::new(&pool);
    ///
    /// // First usage
    /// // SAFETY: We ensure the pool outlives the sender and receiver
    /// let (sender1, receiver1) = unsafe { pinned_pool.bind_by_ptr() };
    /// sender1.send(42);
    /// let value1 = receiver1.await.unwrap();
    /// assert_eq!(value1, 42);
    ///
    /// // Second usage - reuses the same event efficiently
    /// // SAFETY: Pool is still valid and pinned
    /// let (sender2, receiver2) = unsafe { pinned_pool.bind_by_ptr() };
    /// sender2.send(100);
    /// let value2 = receiver2.await.unwrap();
    /// assert_eq!(value2, 100);
    /// // Both sender and receiver pairs are dropped here, before pool
    /// # });
    /// ```
    #[must_use]
    pub unsafe fn bind_by_ptr(
        self: Pin<&Self>,
    ) -> (
        ByPtrPooledLocalOnceSender<T>,
        ByPtrPooledLocalOnceReceiver<T>,
    ) {
        let mut pool_borrow = self.pool.borrow_mut();
        let inserter = pool_borrow.begin_insert();
        let key = inserter.key();
        let _item = inserter.insert_mut(PinnedWithRefCount::new(LocalOnceEvent::new()));

        // Now increment reference count for both sender and receiver
        {
            let mut item_mut = pool_borrow.get_mut(key);
            item_mut.as_mut().inc_ref();
            item_mut.as_mut().inc_ref();
        }

        // Drop the borrow before creating the endpoints
        drop(pool_borrow);

        let pool_ptr: *const Self = self.get_ref();

        (
            ByPtrPooledLocalOnceSender {
                pool: pool_ptr,
                key,
            },
            ByPtrPooledLocalOnceReceiver {
                pool: pool_ptr,
                key: Some(key),
            },
        )
    }

    /// Decrements the reference count for an event and removes it if no longer referenced.
    fn dec_ref_and_cleanup(&self, key: Key) {
        let mut pool_borrow = self.pool.borrow_mut();
        let mut item = pool_borrow.get_mut(key);
        item.as_mut().dec_ref();
        if !item.is_referenced() {
            pool_borrow.remove(key);
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
    /// use events::LocalOnceEventPool;
    ///
    /// let pool = LocalOnceEventPool::<i32>::new();
    ///
    /// // Use the pool which may grow its capacity
    /// for _ in 0..100 {
    ///     let (sender, receiver) = pool.bind_by_ref();
    ///     sender.send(42);
    ///     let _value = futures::executor::block_on(receiver);
    /// }
    ///
    /// // Attempt to shrink to reduce memory usage
    /// pool.shrink_to_fit();
    /// ```
    pub fn shrink_to_fit(&self) {
        let mut pool_borrow = self.pool.borrow_mut();
        pool_borrow.shrink_to_fit();
    }
}

impl<T> Default for LocalOnceEventPool<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::rc::Rc;

    use testing::with_watchdog;

    use super::*;

    #[test]
    fn local_event_pool_by_ref_basic() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let pool = LocalOnceEventPool::new();
                let (sender, receiver) = pool.bind_by_ref();

                sender.send(42);
                let value = receiver.await.unwrap();
                assert_eq!(value, 42);
            });
        });
    }

    #[test]
    fn local_event_pool_by_rc_basic() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let pool = Rc::new(LocalOnceEventPool::new());
                let (sender, receiver) = pool.bind_by_rc(&pool);

                sender.send(42);
                let value = receiver.await.unwrap();
                assert_eq!(value, 42);
            });
        });
    }

    #[test]
    fn local_event_pool_by_ptr_basic() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let pool = LocalOnceEventPool::new();
                let pinned_pool = Pin::new(&pool);
                // SAFETY: We ensure the pool outlives the sender and receiver
                let (sender, receiver) = unsafe { pinned_pool.bind_by_ptr() };

                sender.send(42);
                let value = receiver.await.unwrap();
                assert_eq!(value, 42);
            });
        });
    }

    #[test]
    fn local_event_pool_multiple_events() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let pool = LocalOnceEventPool::new();

                // Test multiple events sequentially
                {
                    let (sender1, receiver1) = pool.bind_by_ref();
                    sender1.send(1);
                    let value1 = receiver1.await.unwrap();
                    assert_eq!(value1, 1);
                }

                {
                    let (sender2, receiver2) = pool.bind_by_ref();
                    sender2.send(2);
                    let value2 = receiver2.await.unwrap();
                    assert_eq!(value2, 2);
                }
            });
        });
    }

    #[test]
    fn local_event_pool_cleanup() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let pool = LocalOnceEventPool::new();

                {
                    let (sender, receiver) = pool.bind_by_ref();
                    sender.send(42);
                    let value = receiver.await.unwrap();
                    assert_eq!(value, 42);
                    // sender and receiver dropped here
                }

                // Pool should be able to shrink after cleanup
                pool.shrink_to_fit();
            });
        });
    }

    #[test]
    fn local_event_pool_rc_multiple_events() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let pool = Rc::new(LocalOnceEventPool::new());

                // Create first event
                let (sender1, receiver1) = pool.bind_by_rc(&pool);

                // Create second event
                let (sender2, receiver2) = pool.bind_by_rc(&pool);

                // Send values
                sender1.send(1);
                sender2.send(2);

                // Receive values
                let value1 = receiver1.await.unwrap();
                let value2 = receiver2.await.unwrap();

                assert_eq!(value1, 1);
                assert_eq!(value2, 2);
            });
        });
    }
}
