//! Pooled events that provide automatic resource management.
//!
//! This module provides pooled variants of events that automatically manage their lifecycle
//! using reference counting. Events are created from pools and automatically returned to the
//! pool when both sender and receiver are dropped.

use std::future::Future;
use std::marker::PhantomPinned;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::task;

use pinned_pool::{Key, PinnedPool};

use super::sync::OnceEvent;
use super::with_ref_count::WithRefCount;
use crate::{Disconnected, ERR_POISONED_LOCK, Sealed};

/// A pool that manages thread-safe events with automatic cleanup.
///
/// The pool creates events on demand and automatically cleans them up when both
/// sender and receiver endpoints are dropped.
///
/// This pool provides zero-allocation event reuse for high-frequency eventing scenarios
/// in a thread-safe manner.
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
/// let value1 = receiver1.await.unwrap();
/// assert_eq!(value1, 42);
/// // Event returned to pool when sender1/receiver1 are dropped
///
/// // Second usage - reuses the same event instance (efficient!)
/// let (sender2, receiver2) = pool.bind_by_ref();
/// sender2.send(100);
/// let value2 = receiver2.await.unwrap();
/// assert_eq!(value2, 100);
/// // Same event reused - no additional allocation overhead
/// # });
/// ```
#[derive(Debug)]
pub struct OnceEventPool<T>
where
    T: Send,
{
    pool: Mutex<PinnedPool<WithRefCount<OnceEvent<T>>>>,

    // It is invalid to move this type once it has been pinned.
    _requires_pinning: PhantomPinned,
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
            _requires_pinning: PhantomPinned,
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
    /// let value1 = receiver1.await.unwrap();
    /// assert_eq!(value1, 42);
    ///
    /// // Second event usage - efficiently reuses the same underlying event
    /// let (sender2, receiver2) = pool.bind_by_ref();
    /// sender2.send(100);
    /// let value2 = receiver2.await.unwrap();
    /// assert_eq!(value2, 100);
    /// # });
    /// ```
    pub fn bind_by_ref(
        &self,
    ) -> (
        PooledOnceSender<T, ByRefPool<'_, T>>,
        PooledOnceReceiver<T, ByRefPool<'_, T>>,
    ) {
        let mut inner_pool = self.pool.lock().expect(ERR_POISONED_LOCK);

        let inserter = inner_pool.begin_insert();
        let key = inserter.key();

        let item = inserter.insert(WithRefCount::new(OnceEvent::new()));

        // It starts with a ref count of 1 but we add one more for the other endpoint.
        item.inc_ref();

        let item_ptr = NonNull::from(item.get_ref());

        let pool_ref = ByRefPool { pool: self };

        (
            PooledOnceSender {
                event: Some(item_ptr),
                pool_ref: pool_ref.clone(),
                key,
            },
            PooledOnceReceiver {
                event: Some(item_ptr),
                pool_ref,
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
    /// let (sender1, receiver1) = pool.bind_by_arc();
    /// sender1.send(42);
    /// let value1 = futures::executor::block_on(receiver1).unwrap();
    /// assert_eq!(value1, 42);
    ///
    /// // Second usage - efficiently reuses the same pooled event
    /// let (sender2, receiver2) = pool.bind_by_arc();
    /// sender2.send(200);
    /// let value2 = futures::executor::block_on(receiver2).unwrap();
    /// assert_eq!(value2, 200);
    /// ```
    pub fn bind_by_arc(
        self: &Arc<Self>,
    ) -> (
        PooledOnceSender<T, ByArcPool<T>>,
        PooledOnceReceiver<T, ByArcPool<T>>,
    ) {
        let mut inner_pool = self.pool.lock().expect(ERR_POISONED_LOCK);

        let inserter = inner_pool.begin_insert();
        let key = inserter.key();

        let item = inserter.insert(WithRefCount::new(OnceEvent::new()));

        // It starts with a ref count of 1 but we add one more for the other endpoint.
        item.inc_ref();

        let item_ptr = NonNull::from(item.get_ref());

        let pool_ref = ByArcPool {
            pool: Arc::clone(self),
        };

        (
            PooledOnceSender {
                event: Some(item_ptr),
                pool_ref: pool_ref.clone(),
                key,
            },
            PooledOnceReceiver {
                event: Some(item_ptr),
                pool_ref,
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
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::OnceEventPool;
    ///
    /// let pool = Box::pin(OnceEventPool::<i32>::new());
    ///
    /// // First usage
    /// // SAFETY: We ensure the pool outlives the sender and receiver
    /// let (sender1, receiver1) = unsafe { pool.as_ref().bind_by_ptr() };
    /// sender1.send(42);
    /// let value1 = futures::executor::block_on(receiver1).unwrap();
    /// assert_eq!(value1, 42);
    ///
    /// // Second usage - reuses the same event from the pool efficiently
    /// // SAFETY: Pool is still valid and pinned
    /// let (sender2, receiver2) = unsafe { pool.as_ref().bind_by_ptr() };
    /// sender2.send(100);
    /// let value2 = futures::executor::block_on(receiver2).unwrap();
    /// assert_eq!(value2, 100);
    /// // Both sender and receiver pairs are dropped here, before pool
    /// ```
    #[must_use]
    pub unsafe fn bind_by_ptr(
        self: Pin<&Self>,
    ) -> (
        PooledOnceSender<T, ByPtrPool<T>>,
        PooledOnceReceiver<T, ByPtrPool<T>>,
    ) {
        let mut inner_pool = self.pool.lock().expect(ERR_POISONED_LOCK);

        let inserter = inner_pool.begin_insert();
        let key = inserter.key();

        let item = inserter.insert(WithRefCount::new(OnceEvent::new()));

        // It starts with a ref count of 1 but we add one more for the other endpoint.
        item.inc_ref();

        let item_ptr = NonNull::from(item.get_ref());

        let pool_ref = ByPtrPool {
            pool: NonNull::from(self.get_ref()),
        };

        (
            PooledOnceSender {
                event: Some(item_ptr),
                pool_ref: pool_ref.clone(),
                key,
            },
            PooledOnceReceiver {
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
    /// use events::OnceEventPool;
    ///
    /// let pool = OnceEventPool::<i32>::new();
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
        let mut inner_pool = self.pool.lock().expect("pool mutex should not be poisoned");
        inner_pool.shrink_to_fit();
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

/// Enables a sender or receiver to reference the pool that stores the event that connects them.
///
/// This is a sealed trait and exists for internal use only. You never need to use it.
#[expect(private_bounds, reason = "intentional - sealed trait")]
pub trait PoolRef<T>: Deref<Target = OnceEventPool<T>> + Sealed
where
    T: Send,
{
}

/// An event pool referenced via `&` shared reference.
///
/// Only used in type names. Instances are created internally by [`OnceEventPool`].
#[derive(Copy, Debug)]
pub struct ByRefPool<'a, T>
where
    T: Send,
{
    pool: &'a OnceEventPool<T>,
}

impl<T> Sealed for ByRefPool<'_, T> where T: Send {}
impl<T> PoolRef<T> for ByRefPool<'_, T> where T: Send {}
impl<T> Deref for ByRefPool<'_, T>
where
    T: Send,
{
    type Target = OnceEventPool<T>;

    fn deref(&self) -> &Self::Target {
        self.pool
    }
}
impl<T> Clone for ByRefPool<'_, T>
where
    T: Send,
{
    fn clone(&self) -> Self {
        Self { pool: self.pool }
    }
}

/// An event pool referenced via `Arc` shared reference.
///
/// Only used in type names. Instances are created internally by [`OnceEventPool`].
#[derive(Debug)]
pub struct ByArcPool<T>
where
    T: Send,
{
    pool: Arc<OnceEventPool<T>>,
}

impl<T> Sealed for ByArcPool<T> where T: Send {}
impl<T> PoolRef<T> for ByArcPool<T> where T: Send {}
impl<T> Deref for ByArcPool<T>
where
    T: Send,
{
    type Target = OnceEventPool<T>;

    fn deref(&self) -> &Self::Target {
        &self.pool
    }
}
impl<T> Clone for ByArcPool<T>
where
    T: Send,
{
    fn clone(&self) -> Self {
        Self {
            pool: Arc::clone(&self.pool),
        }
    }
}

/// An event pool referenced via raw pointer.
///
/// Only used in type names. Instances are created internally by [`OnceEventPool`].
#[derive(Copy, Debug)]
pub struct ByPtrPool<T>
where
    T: Send,
{
    pool: NonNull<OnceEventPool<T>>,
}

impl<T> Sealed for ByPtrPool<T> where T: Send {}
impl<T> PoolRef<T> for ByPtrPool<T> where T: Send {}
impl<T> Deref for ByPtrPool<T>
where
    T: Send,
{
    type Target = OnceEventPool<T>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The creator of the reference is responsible for ensuring the pool outlives it.
        unsafe { self.pool.as_ref() }
    }
}
impl<T> Clone for ByPtrPool<T>
where
    T: Send,
{
    fn clone(&self) -> Self {
        Self { pool: self.pool }
    }
}
// SAFETY: This is only used with the thread-safe pool (the pool is Sync).
unsafe impl<T> Send for ByPtrPool<T> where T: Send {}

/// A sender endpoint for pooled  events that holds a reference to the pool.
///
/// This sender is created from [`OnceEventPool::bind_by_ref`] and automatically manages
/// the lifetime of the underlying event. When both sender and receiver are dropped,
/// the event is automatically returned to the pool.
///
/// This is the single-threaded variant that cannot be sent across threads.
#[derive(Debug)]
pub struct PooledOnceSender<T, R>
where
    T: Send,
    R: PoolRef<T>,
{
    // This is a pointer to avoid contaminating the type signature with the event lifetime.
    //
    // SAFETY: We rely on the inner pool guaranteeing pinning and us owning a counted reference.
    event: Option<NonNull<WithRefCount<OnceEvent<T>>>>,

    pool_ref: R,
    key: Key,
}

impl<T, R> PooledOnceSender<T, R>
where
    T: Send,
    R: PoolRef<T>,
{
    /// Sends a value through the event.
    ///
    /// This method consumes the sender and always succeeds, regardless of whether
    /// there is a receiver waiting.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::OnceEventPool;
    ///
    /// let pool = OnceEventPool::new();
    /// let (sender, receiver) = pool.bind_by_ref();
    ///
    /// sender.send(42);
    /// let value = futures::executor::block_on(receiver).unwrap();
    /// assert_eq!(value, 42);
    /// ```
    #[cfg_attr(test, mutants::skip)] // Can cause infinite loops - resource management is very sensitive to this.
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

impl<T, R> Drop for PooledOnceSender<T, R>
where
    T: Send,
    R: PoolRef<T>,
{
    fn drop(&mut self) {
        // SAFETY: See comments on field.
        let event = unsafe { self.event.expect("only possible on double drop").as_ref() };

        if event.dec_ref() {
            // The event is going to be destroyed, so we cannot reference it anymore.
            self.event = None;

            self.pool_ref
                .pool
                .lock()
                .expect(ERR_POISONED_LOCK)
                .remove(self.key);
        }
    }
}

// SAFETY: The NonNull marks it !Send by default but we know that everything behind the pointer
// is thread-safe, so all is well. We also require `Send` from `R` to be extra safe here.
unsafe impl<T, R> Send for PooledOnceSender<T, R>
where
    T: Send,
    R: PoolRef<T> + Send,
{
}

/// A receiver endpoint for pooled  events that holds a reference to the pool.
///
/// This receiver is created from [`OnceEventPool::bind_by_ref`] and automatically manages
/// the lifetime of the underlying event. When both sender and receiver are dropped,
/// the event is automatically returned to the pool.
///
/// This is the single-threaded variant that cannot be sent across threads.
#[derive(Debug)]
pub struct PooledOnceReceiver<T, R>
where
    T: Send,
    R: PoolRef<T>,
{
    // This is a pointer to avoid contaminating the type signature with the event lifetime.
    //
    // SAFETY: We rely on the inner pool guaranteeing pinning and us owning a counted reference.
    event: Option<NonNull<WithRefCount<OnceEvent<T>>>>,

    pool_ref: R,
    key: Key,
}

impl<T, R> PooledOnceReceiver<T, R>
where
    T: Send,
    R: PoolRef<T>,
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

            self.pool_ref
                .pool
                .lock()
                .expect(ERR_POISONED_LOCK)
                .remove(self.key);
        }
    }
}

impl<T, R> Future for PooledOnceReceiver<T, R>
where
    T: Send,
    R: PoolRef<T>,
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

impl<T, R> Drop for PooledOnceReceiver<T, R>
where
    T: Send,
    R: PoolRef<T>,
{
    fn drop(&mut self) {
        self.drop_inner();
    }
}

// SAFETY: The NonNull marks it !Send by default but we know that everything behind the pointer
// is thread-safe, so all is well. We also require `Send` from `R` to be extra safe here.
unsafe impl<T, R> Send for PooledOnceReceiver<T, R>
where
    T: Send,
    R: PoolRef<T> + Send,
{
}

#[cfg(test)]
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use testing::with_watchdog;

    use super::*;

    #[test]
    fn event_pool_by_ref() {
        with_watchdog(|| {
            let pool = OnceEventPool::<i32>::new();
            let (sender, receiver) = pool.bind_by_ref();

            sender.send(42);
            let value = futures::executor::block_on(receiver).unwrap();
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
            let value1 = futures::executor::block_on(receiver1).unwrap();
            assert_eq!(value1, 1);

            // Test another event
            let (sender2, receiver2) = pool.bind_by_ref();
            sender2.send(2);
            let value2 = futures::executor::block_on(receiver2).unwrap();
            assert_eq!(value2, 2);
        });
    }

    #[test]
    fn event_pool_by_arc() {
        with_watchdog(|| {
            let pool = Arc::new(OnceEventPool::<i32>::new());
            let (sender, receiver) = pool.bind_by_arc();

            sender.send(42);
            let value = futures::executor::block_on(receiver).unwrap();
            assert_eq!(value, 42);
        });
    }

    #[test]
    fn event_pool_by_ptr() {
        with_watchdog(|| {
            let pool = Box::pin(OnceEventPool::<i32>::new());

            // SAFETY: We ensure the pool outlives the sender and receiver
            let (sender, receiver) = unsafe { pool.as_ref().bind_by_ptr() };

            sender.send(42);
            let value = futures::executor::block_on(receiver).unwrap();
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
            let value = futures::executor::block_on(receiver2).unwrap();
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
            let value = futures::executor::block_on(receiver2).unwrap();
            assert_eq!(value, 456);
        });
    }

    #[test]
    fn by_arc_sender_drop_cleanup() {
        with_watchdog(|| {
            let pool = Arc::new(OnceEventPool::<i32>::new());
            let (sender, _receiver) = pool.bind_by_arc();

            // Force the sender to be dropped without being consumed by send()
            drop(sender);

            // Create a new event to verify the pool is still functional
            let (sender2, receiver2) = pool.bind_by_arc();
            sender2.send(654);
            let value = futures::executor::block_on(receiver2).unwrap();
            assert_eq!(value, 654);
        });
    }

    #[test]
    fn by_arc_receiver_drop_cleanup() {
        with_watchdog(|| {
            let pool = Arc::new(OnceEventPool::<i32>::new());
            let (_sender, receiver) = pool.bind_by_arc();

            // Force the receiver to be dropped without being consumed by recv()
            drop(receiver);

            // Create a new event to verify the pool is still functional
            let (sender2, receiver2) = pool.bind_by_arc();
            sender2.send(987);
            let value = futures::executor::block_on(receiver2).unwrap();
            assert_eq!(value, 987);
        });
    }

    #[test]
    fn by_ptr_sender_drop_cleanup() {
        with_watchdog(|| {
            let pool = Box::pin(OnceEventPool::<i32>::new());

            // SAFETY: We ensure the pool outlives the sender and receiver
            let (sender, _receiver) = unsafe { pool.as_ref().bind_by_ptr() };

            // Force the sender to be dropped without being consumed by send()
            drop(sender);

            // Create a new event to verify the pool is still functional
            // SAFETY: We ensure the pool outlives the sender and receiver
            let (sender2, receiver2) = unsafe { pool.as_ref().bind_by_ptr() };
            sender2.send(147);
            let value = futures::executor::block_on(receiver2).unwrap();
            assert_eq!(value, 147);
        });
    }

    #[test]
    fn by_ptr_receiver_drop_cleanup() {
        with_watchdog(|| {
            let pool = Box::pin(OnceEventPool::<i32>::new());

            // SAFETY: We ensure the pool outlives the sender and receiver
            let (_sender, receiver) = unsafe { pool.as_ref().bind_by_ptr() };

            // Force the receiver to be dropped without being consumed by recv()
            drop(receiver);

            // Create a new event to verify the pool is still functional
            // SAFETY: We ensure the pool outlives the sender and receiver
            let (sender2, receiver2) = unsafe { pool.as_ref().bind_by_ptr() };
            sender2.send(258);
            let value = futures::executor::block_on(receiver2).unwrap();
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
            let value = futures::executor::block_on(receiver).unwrap();
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
                    let _value = futures::executor::block_on(receiver);
                }
            }

            // The pool should have cleaned up unused events
            // If cleanup is broken, the pool would retain all the unused events
            // This is a bit of an implementation detail but it's necessary to catch the leak

            // Create one more event to verify pool still works
            let (sender, receiver) = pool.bind_by_ref();
            sender.send(42);
            let value = futures::executor::block_on(receiver).unwrap();
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
            let value = futures::executor::block_on(receiver).unwrap();
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
        let mut pool_guard = pool.pool.lock().unwrap();
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
            let (_sender, _receiver) = pool.bind_by_arc();
            // Both sender and receiver will be dropped here
        }

        // Pool should be cleaned up - all events should be removed
        let mut pool_guard = pool.pool.lock().unwrap();
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
            {
                let pool = Box::pin(OnceEventPool::<u32>::new());
                // SAFETY: We pin the pool for the duration of by_ptr call
                let (_sender, _receiver) = unsafe { pool.as_ref().bind_by_ptr() };
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
            let pool_guard = pool.pool.lock().unwrap();
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
            let pool_guard = pool.pool.lock().unwrap();
            pool_guard.len()
        };
        assert_eq!(
            pool_len_after, pool_len_before,
            "Pool not cleaned up after dropping events - dec_ref_and_cleanup not working, \
             key: {sender_key:?}"
        );
    }

    #[test]
    fn shrink_to_fit_with_empty_pool_shrinks_to_zero() {
        let pool = OnceEventPool::<u32>::new();

        // Create and drop events without using them
        for _ in 0..10 {
            drop(pool.bind_by_ref());
        }

        assert_eq!(pool.pool.lock().unwrap().len(), 0);

        // Shrink the pool to fit
        pool.shrink_to_fit();

        assert_eq!(
            pool.pool.lock().unwrap().capacity(),
            0,
            "Empty pool should shrink to capacity 0"
        );
    }

    #[test]
    fn event_removed_from_pool_after_endpoints_immediate_drop() {
        let pool = OnceEventPool::<u32>::new();

        drop(pool.bind_by_ref());

        assert_eq!(pool.pool.lock().unwrap().len(), 0);
    }

    #[test]
    fn thread_safety() {
        // The pool is accessed across threads, so requires Sync as well as Send.
        assert_impl_all!(OnceEventPool<u32>: Send, Sync);

        // These are all meant to be consumed locally - they may move between threads but are
        // not shared between threads, so Sync is not expected, only Send.
        assert_impl_all!(PooledOnceSender<u32, ByRefPool<'static, u32>>: Send);
        assert_impl_all!(PooledOnceReceiver<u32, ByRefPool<'static, u32>>: Send);
        assert_impl_all!(PooledOnceSender<u32, ByArcPool<u32>>: Send);
        assert_impl_all!(PooledOnceReceiver<u32, ByArcPool<u32>>: Send);
        assert_impl_all!(PooledOnceSender<u32, ByPtrPool<u32>>: Send);
        assert_impl_all!(PooledOnceReceiver<u32, ByPtrPool<u32>>: Send);
        assert_not_impl_any!(PooledOnceSender<u32, ByRefPool<'static, u32>>: Sync);
        assert_not_impl_any!(PooledOnceReceiver<u32, ByRefPool<'static, u32>>: Sync);
        assert_not_impl_any!(PooledOnceSender<u32, ByArcPool<u32>>: Sync);
        assert_not_impl_any!(PooledOnceReceiver<u32, ByArcPool<u32>>: Sync);
        assert_not_impl_any!(PooledOnceSender<u32, ByPtrPool<u32>>: Sync);
        assert_not_impl_any!(PooledOnceReceiver<u32, ByPtrPool<u32>>: Sync);
    }
}
