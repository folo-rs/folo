//! Pooled local events that provide automatic resource management for single-threaded use.
//!
//! This module provides pooled variants of local events that automatically manage their lifecycle
//! using reference counting. Events are created from pools and automatically returned to the
//! pool when both sender and receiver are dropped.

use std::cell::RefCell;
use std::marker::PhantomPinned;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::Rc;
use std::task;

use pinned_pool::{Key, PinnedPool};

use super::local::LocalOnceEvent;
use super::with_ref_count::LocalWithRefCount;
use crate::{Disconnected, Sealed};

/// A pool that manages single-threaded events with automatic cleanup.
///
/// The pool creates local events on demand and automatically cleans them up when both
/// sender and receiver endpoints are dropped.
///
/// This is the single-threaded variant that cannot be shared across threads but has
/// lower overhead than the thread-safe [`super::OnceEventPool`].
///
/// The pool provides zero-allocation event reuse for high-frequency eventing scenarios
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
    pool: RefCell<PinnedPool<LocalWithRefCount<LocalOnceEvent<T>>>>,

    // It is invalid to move this type once it has been pinned.
    _requires_pinning: PhantomPinned,
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
            _requires_pinning: PhantomPinned,
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
        PooledLocalOnceSender<T, ByRefLocalPool<'_, T>>,
        PooledLocalOnceReceiver<T, ByRefLocalPool<'_, T>>,
    ) {
        let mut inner_pool = self.pool.borrow_mut();
        let inserter = inner_pool.begin_insert();
        let key = inserter.key();

        let item = inserter.insert(LocalWithRefCount::new(LocalOnceEvent::new()));

        // It starts with a ref count of 1 but we add one more for the other endpoint.
        item.inc_ref();

        let item_ptr = NonNull::from(item.get_ref());

        let pool_ref = ByRefLocalPool { pool: self };

        (
            PooledLocalOnceSender {
                event: Some(item_ptr),
                pool_ref: pool_ref.clone(),
                key,
            },
            PooledLocalOnceReceiver {
                event: Some(item_ptr),
                pool_ref,
                key,
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
    /// let (sender1, receiver1) = pool.bind_by_rc();
    /// sender1.send(42);
    /// let value1 = futures::executor::block_on(receiver1).unwrap();
    /// assert_eq!(value1, 42);
    ///
    /// // Second usage - reuses the same event from the pool
    /// let (sender2, receiver2) = pool.bind_by_rc();
    /// sender2.send(200);
    /// let value2 = futures::executor::block_on(receiver2).unwrap();
    /// assert_eq!(value2, 200);
    /// ```
    pub fn bind_by_rc(
        self: &Rc<Self>,
    ) -> (
        PooledLocalOnceSender<T, ByRcLocalPool<T>>,
        PooledLocalOnceReceiver<T, ByRcLocalPool<T>>,
    ) {
        let mut inner_pool = self.pool.borrow_mut();
        let inserter = inner_pool.begin_insert();
        let key = inserter.key();
        let item = inserter.insert(LocalWithRefCount::new(LocalOnceEvent::new()));

        // It starts with a ref count of 1 but we add one more for the other endpoint.
        item.inc_ref();

        let item_ptr = NonNull::from(item.get_ref());

        let pool_ref = ByRcLocalPool {
            pool: Rc::clone(self),
        };

        (
            PooledLocalOnceSender {
                event: Some(item_ptr),
                pool_ref: pool_ref.clone(),
                key,
            },
            PooledLocalOnceReceiver {
                event: Some(item_ptr),
                pool_ref,
                key,
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
    /// let pool = Box::pin(LocalOnceEventPool::<i32>::new());
    ///
    /// // First usage
    /// // SAFETY: We ensure the pool outlives the sender and receiver
    /// let (sender1, receiver1) = unsafe { pool.as_ref().bind_by_ptr() };
    /// sender1.send(42);
    /// let value1 = receiver1.await.unwrap();
    /// assert_eq!(value1, 42);
    ///
    /// // Second usage - reuses the same event efficiently
    /// // SAFETY: Pool is still valid and pinned
    /// let (sender2, receiver2) = unsafe { pool.as_ref().bind_by_ptr() };
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
        PooledLocalOnceSender<T, ByPtrLocalPool<T>>,
        PooledLocalOnceReceiver<T, ByPtrLocalPool<T>>,
    ) {
        let mut inner_pool = self.pool.borrow_mut();
        let inserter = inner_pool.begin_insert();
        let key = inserter.key();

        let item = inserter.insert(LocalWithRefCount::new(LocalOnceEvent::new()));

        // It starts with a ref count of 1 but we add one more for the other endpoint.
        item.inc_ref();

        let item_ptr = NonNull::from(item.get_ref());

        let pool_ref = ByPtrLocalPool {
            pool: NonNull::from(self.get_ref()),
        };

        (
            PooledLocalOnceSender {
                event: Some(item_ptr),
                pool_ref: pool_ref.clone(),
                key,
            },
            PooledLocalOnceReceiver {
                event: Some(item_ptr),
                pool_ref,
                key,
            },
        )
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
        let mut inner_pool = self.pool.borrow_mut();
        inner_pool.shrink_to_fit();
    }
}

impl<T> Default for LocalOnceEventPool<T> {
    fn default() -> Self {
        Self::new()
    }
}

/// Enables a sender or receiver to reference the pool that stores the event that connects them.
/// 
/// This is a sealed trait and exists for internal use only. You never need to use it.
#[expect(private_bounds, reason = "intentional - sealed trait")]
pub trait LocalPoolRef<T>: Deref<Target = LocalOnceEventPool<T>> + Sealed {}

/// An event pool referenced via `&` shared reference.
///
/// Only used in type names. Instances are created internally by [`LocalOnceEventPool`].
#[derive(Copy, Debug)]
pub struct ByRefLocalPool<'a, T> {
    pool: &'a LocalOnceEventPool<T>,
}

impl<T> Sealed for ByRefLocalPool<'_, T> {}
impl<T> LocalPoolRef<T> for ByRefLocalPool<'_, T> {}
impl<T> Deref for ByRefLocalPool<'_, T> {
    type Target = LocalOnceEventPool<T>;

    fn deref(&self) -> &Self::Target {
        self.pool
    }
}
impl<T> Clone for ByRefLocalPool<'_, T> {
    fn clone(&self) -> Self {
        Self { pool: self.pool }
    }
}

/// An event pool referenced via `Rc` shared reference.
///
/// Only used in type names. Instances are created internally by [`LocalOnceEventPool`].
#[derive(Debug)]
pub struct ByRcLocalPool<T> {
    pool: Rc<LocalOnceEventPool<T>>,
}

impl<T> Sealed for ByRcLocalPool<T> {}
impl<T> LocalPoolRef<T> for ByRcLocalPool<T> {}
impl<T> Deref for ByRcLocalPool<T> {
    type Target = LocalOnceEventPool<T>;

    fn deref(&self) -> &Self::Target {
        &self.pool
    }
}
impl<T> Clone for ByRcLocalPool<T> {
    fn clone(&self) -> Self {
        Self {
            pool: Rc::clone(&self.pool),
        }
    }
}

/// An event pool referenced via raw pointer.
///
/// Only used in type names. Instances are created internally by [`LocalOnceEventPool`].
#[derive(Copy, Debug)]
pub struct ByPtrLocalPool<T> {
    pool: NonNull<LocalOnceEventPool<T>>,
}

impl<T> Sealed for ByPtrLocalPool<T> {}
impl<T> LocalPoolRef<T> for ByPtrLocalPool<T> {}
impl<T> Deref for ByPtrLocalPool<T> {
    type Target = LocalOnceEventPool<T>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The creator of the reference is responsible for ensuring the pool outlives it.
        unsafe { self.pool.as_ref() }
    }
}
impl<T> Clone for ByPtrLocalPool<T> {
    fn clone(&self) -> Self {
        Self { pool: self.pool }
    }
}

/// A sender endpoint for pooled local events that holds a reference to the pool.
///
/// This sender is created from [`LocalOnceEventPool::bind_by_ref`] and automatically manages
/// the lifetime of the underlying event. When both sender and receiver are dropped,
/// the event is automatically returned to the pool.
///
/// This is the single-threaded variant that cannot be sent across threads.
#[derive(Debug)]
pub struct PooledLocalOnceSender<T, R>
where
    R: LocalPoolRef<T>,
{
    // This is a pointer to avoid contaminating the type signature with the event lifetime.
    //
    // SAFETY: We rely on the inner pool guaranteeing pinning and us owning a counted reference.
    event: Option<NonNull<LocalWithRefCount<LocalOnceEvent<T>>>>,

    pool_ref: R,
    key: Key,
}

impl<T, R> PooledLocalOnceSender<T, R>
where
    R: LocalPoolRef<T>,
{
    /// Sends a value through the event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::LocalOnceEventPool;
    ///
    /// let pool = LocalOnceEventPool::new();
    /// let (sender, receiver) = pool.bind_by_ref();
    ///
    /// sender.send(42);
    /// let value = futures::executor::block_on(receiver).unwrap();
    /// assert_eq!(value, 42);
    /// ```
    pub fn send(self, value: T) {
        // SAFETY: See comments on field.
        let event = unsafe {
            self.event
                .expect("event is only None during destruction")
                .as_ref()
        };

        event.set(value);
    }
}

impl<T, R> Drop for PooledLocalOnceSender<T, R>
where
    R: LocalPoolRef<T>,
{
    fn drop(&mut self) {
        // SAFETY: See comments on field.
        let event = unsafe { self.event.expect("only possible on double drop").as_ref() };

        if event.dec_ref() {
            // The event is going to be destroyed, so we cannot reference it anymore.
            self.event = None;

            self.pool_ref.pool.borrow_mut().remove(self.key);
        }
    }
}

/// A receiver endpoint for pooled local events that holds a reference to the pool.
///
/// This receiver is created from [`LocalOnceEventPool::bind_by_ref`] and automatically manages
/// the lifetime of the underlying event. When both sender and receiver are dropped,
/// the event is automatically returned to the pool.
///
/// This is the single-threaded variant that cannot be sent across threads.
#[derive(Debug)]
pub struct PooledLocalOnceReceiver<T, R>
where
    R: LocalPoolRef<T>,
{
    // This is a pointer to avoid contaminating the type signature with the event lifetime.
    //
    // SAFETY: We rely on the inner pool guaranteeing pinning and us owning a counted reference.
    event: Option<NonNull<LocalWithRefCount<LocalOnceEvent<T>>>>,

    pool_ref: R,
    key: Key,
}

impl<T, R> PooledLocalOnceReceiver<T, R>
where
    R: LocalPoolRef<T>,
{
    /// Drops the inner state, releasing the event back to the pool.
    /// May also be used from contexts where the receiver itself is not yet consumed.
    fn drop_inner(&mut self) {
        let Some(event) = self.event else {
            // Already pseudo-consumed the receiver as part of the Future impl.
            return;
        };

        // SAFETY: See comments on field.
        let event = unsafe { event.as_ref() };

        if event.dec_ref() {
            // The event is going to be destroyed, so we cannot reference it anymore.
            self.event = None;

            self.pool_ref.pool.borrow_mut().remove(self.key);
        }
    }
}

impl<T, R> Future for PooledLocalOnceReceiver<T, R>
where
    R: LocalPoolRef<T>,
{
    type Output = Result<T, Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut task::Context<'_>) -> task::Poll<Self::Output> {
        // SAFETY: We are not moving anything, just touching internal state.
        let this = unsafe { self.get_unchecked_mut() };

        // SAFETY: See comments on field.
        let event = unsafe {
            this.event
                .expect("polling a Future after completion is invalid")
                .as_ref()
        };

        let poll_result = event.poll(cx.waker());

        poll_result.map_or_else(
            || task::Poll::Pending,
            |value| {
                this.drop_inner();
                task::Poll::Ready(value)
            },
        )
    }
}

impl<T, R> Drop for PooledLocalOnceReceiver<T, R>
where
    R: LocalPoolRef<T>,
{
    fn drop(&mut self) {
        self.drop_inner();
    }
}

#[cfg(test)]
mod tests {
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
                let (sender, receiver) = pool.bind_by_rc();

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
                let pool = Box::pin(LocalOnceEventPool::new());

                // SAFETY: We ensure the pool outlives the sender and receiver
                let (sender, receiver) = unsafe { pool.as_ref().bind_by_ptr() };

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
                let (sender1, receiver1) = pool.bind_by_rc();

                // Create second event
                let (sender2, receiver2) = pool.bind_by_rc();

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
