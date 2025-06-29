//! Pooled local events that provide automatic resource management for single-threaded use.
//!
//! This module provides pooled variants of local events that automatically manage their lifecycle
//! using reference counting. Events are created from pools and automatically returned to the
//! pool when both sender and receiver are dropped.

use std::cell::RefCell;
use std::pin::Pin;
use std::rc::Rc;

use pinned_pool::{Key, PinnedPool};

use super::local::LocalEvent;

mod by_ptr;
mod by_rc;
mod by_ref;

pub use by_ptr::{ByPtrPooledLocalEventReceiver, ByPtrPooledLocalEventSender};
pub use by_rc::{ByRcPooledLocalEventReceiver, ByRcPooledLocalEventSender};
pub use by_ref::{ByRefPooledLocalEventReceiver, ByRefPooledLocalEventSender};

/// Just combines a value and a reference count, for use in custom reference counting logic.
/// This is the single-threaded variant that uses `RefCell` instead of `Mutex`.
#[derive(Debug)]
pub struct WithRefCountLocal<T> {
    value: T,
    ref_count: usize,
}

impl<T> WithRefCountLocal<T> {
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

impl<T> Default for WithRefCountLocal<T>
where
    T: Default,
{
    fn default() -> Self {
        Self::new(T::default())
    }
}

/// A pool that manages single-threaded events with automatic cleanup.
///
/// The pool creates local events on demand and automatically cleans them up when both
/// sender and receiver endpoints are dropped. Events are reference-counted to
/// track when they are no longer in use.
///
/// This is the single-threaded variant that cannot be shared across threads but has
/// lower overhead than the thread-safe [`super::EventPool`].
///
/// # Example
///
/// ```rust
/// use events::once::LocalEventPool;
///
/// let mut pool = LocalEventPool::<i32>::new();
/// let (sender, receiver) = pool.by_ref();
///
/// sender.send(42);
/// let value = futures::executor::block_on(receiver);
/// assert_eq!(value, 42);
/// // Event is automatically returned to pool when sender/receiver are dropped
/// ```
#[derive(Debug)]
pub struct LocalEventPool<T> {
    pool: PinnedPool<WithRefCountLocal<LocalEvent<T>>>,
}

impl<T> LocalEventPool<T> {
    /// Creates a new empty local event pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::once::LocalEventPool;
    ///
    /// let pool = LocalEventPool::<String>::new();
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            pool: PinnedPool::new(),
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
    /// use events::once::LocalEventPool;
    ///
    /// let mut pool = LocalEventPool::<i32>::new();
    /// let (sender, receiver) = pool.by_ref();
    ///
    /// sender.send(42);
    /// let value = futures::executor::block_on(receiver);
    /// assert_eq!(value, 42);
    /// ```
    pub fn by_ref(
        &mut self,
    ) -> (
        ByRefPooledLocalEventSender<'_, T>,
        ByRefPooledLocalEventReceiver<'_, T>,
    ) {
        let inserter = self.pool.begin_insert();
        let key = inserter.key();
        let _item = inserter.insert_mut(WithRefCountLocal::new(LocalEvent::new()));

        // Increment reference count for both sender and receiver
        let item_mut = self.pool.get_mut(key);
        // SAFETY: WithRefCountLocal doesn't contain self-references so it's safe to get_unchecked_mut
        unsafe {
            let item_ref = item_mut.get_unchecked_mut();
            item_ref.inc_ref();
            item_ref.inc_ref();
        }

        // Use raw pointer to avoid borrowing self twice
        let pool_ptr: *mut Self = self;

        (
            ByRefPooledLocalEventSender {
                pool: pool_ptr,
                key,
                _phantom: std::marker::PhantomData,
            },
            ByRefPooledLocalEventReceiver {
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
    /// use events::once::LocalEventPool;
    ///
    /// let pool = Rc::new(std::cell::RefCell::new(LocalEventPool::<i32>::new()));
    /// let (sender, receiver) = pool.borrow_mut().by_rc(&pool);
    ///
    /// sender.send(42);
    /// let value = futures::executor::block_on(receiver);
    /// assert_eq!(value, 42);
    /// ```
    pub fn by_rc(
        &mut self,
        pool_rc: &Rc<RefCell<Self>>,
    ) -> (
        ByRcPooledLocalEventSender<T>,
        ByRcPooledLocalEventReceiver<T>,
    ) {
        let inserter = self.pool.begin_insert();
        let key = inserter.key();
        let _item = inserter.insert_mut(WithRefCountLocal::new(LocalEvent::new()));

        // Now increment reference count for both sender and receiver
        let item_mut = self.pool.get_mut(key);
        // SAFETY: WithRefCountLocal doesn't contain self-references so it's safe to get_unchecked_mut
        unsafe {
            let item_ref = item_mut.get_unchecked_mut();
            item_ref.inc_ref();
            item_ref.inc_ref();
        }

        (
            ByRcPooledLocalEventSender {
                pool: Rc::clone(pool_rc),
                key,
            },
            ByRcPooledLocalEventReceiver {
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
    /// use events::once::LocalEventPool;
    ///
    /// let mut pool = LocalEventPool::<i32>::new();
    /// let pinned_pool = Pin::new(&mut pool);
    /// // SAFETY: We ensure the pool outlives the sender and receiver
    /// let (sender, receiver) = unsafe { pinned_pool.by_ptr() };
    ///
    /// sender.send(42);
    /// let value = receiver.await;
    /// assert_eq!(value, 42);
    /// // sender and receiver are dropped here, before pool
    /// ```
    #[must_use]
    pub unsafe fn by_ptr(
        self: Pin<&mut Self>,
    ) -> (
        ByPtrPooledLocalEventSender<T>,
        ByPtrPooledLocalEventReceiver<T>,
    ) {
        // SAFETY: We need to access the mutable reference to create the event
        // The caller guarantees the pool remains pinned and valid
        let this = unsafe { self.get_unchecked_mut() };

        let inserter = this.pool.begin_insert();
        let key = inserter.key();
        let _item = inserter.insert_mut(WithRefCountLocal::new(LocalEvent::new()));

        // Now increment reference count for both sender and receiver
        let item_mut = this.pool.get_mut(key);
        // SAFETY: WithRefCountLocal doesn't contain self-references so it's safe to get_unchecked_mut
        unsafe {
            let item_ref = item_mut.get_unchecked_mut();
            item_ref.inc_ref();
            item_ref.inc_ref();
        }

        let pool_ptr: *mut Self = this;

        (
            ByPtrPooledLocalEventSender {
                pool: pool_ptr,
                key,
            },
            ByPtrPooledLocalEventReceiver {
                pool: pool_ptr,
                key: Some(key),
            },
        )
    }

    /// Decrements the reference count for an event and removes it if no longer referenced.
    fn dec_ref_and_cleanup(&mut self, key: Key) {
        let item = self.pool.get_mut(key);
        // SAFETY: WithRefCountLocal doesn't contain self-references so it's safe to get_unchecked_mut
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
    /// use events::once::LocalEventPool;
    ///
    /// let mut pool = LocalEventPool::<i32>::new();
    ///
    /// // Use the pool which may grow its capacity
    /// for _ in 0..100 {
    ///     let (sender, receiver) = pool.by_ref();
    ///     sender.send(42);
    ///     let _value = futures::executor::block_on(receiver);
    /// }
    ///
    /// // Attempt to shrink to reduce memory usage
    /// pool.shrink_to_fit();
    /// ```
    pub fn shrink_to_fit(&mut self) {
        self.pool.shrink_to_fit();
    }
}

impl<T> Default for LocalEventPool<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::pin::Pin;
    use std::rc::Rc;

    use testing::with_watchdog;

    use super::*;

    #[test]
    fn local_event_pool_by_ref_basic() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let mut pool = LocalEventPool::new();
                let (sender, receiver) = pool.by_ref();

                sender.send(42);
                let value = receiver.await;
                assert_eq!(value, 42);
            });
        });
    }

    #[test]
    fn local_event_pool_by_rc_basic() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let pool = Rc::new(RefCell::new(LocalEventPool::new()));
                let (sender, receiver) = pool.borrow_mut().by_rc(&pool);

                sender.send(42);
                let value = receiver.await;
                assert_eq!(value, 42);
            });
        });
    }

    #[test]
    fn local_event_pool_by_ptr_basic() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let mut pool = LocalEventPool::new();
                let pinned_pool = Pin::new(&mut pool);
                // SAFETY: We ensure the pool outlives the sender and receiver
                let (sender, receiver) = unsafe { pinned_pool.by_ptr() };

                sender.send(42);
                let value = receiver.await;
                assert_eq!(value, 42);
            });
        });
    }

    #[test]
    fn local_event_pool_multiple_events() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let mut pool = LocalEventPool::new();

                // Test multiple events sequentially
                {
                    let (sender1, receiver1) = pool.by_ref();
                    sender1.send(1);
                    let value1 = receiver1.await;
                    assert_eq!(value1, 1);
                }

                {
                    let (sender2, receiver2) = pool.by_ref();
                    sender2.send(2);
                    let value2 = receiver2.await;
                    assert_eq!(value2, 2);
                }
            });
        });
    }

    #[test]
    fn local_event_pool_cleanup() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let mut pool = LocalEventPool::new();

                {
                    let (sender, receiver) = pool.by_ref();
                    sender.send(42);
                    let value = receiver.await;
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
                let pool = Rc::new(RefCell::new(LocalEventPool::new()));

                // Create first event
                let (sender1, receiver1) = pool.borrow_mut().by_rc(&pool);

                // Create second event
                let (sender2, receiver2) = pool.borrow_mut().by_rc(&pool);

                // Send values
                sender1.send(1);
                sender2.send(2);

                // Receive values
                let value1 = receiver1.await;
                let value2 = receiver2.await;

                assert_eq!(value1, 1);
                assert_eq!(value2, 2);
            });
        });
    }
}
