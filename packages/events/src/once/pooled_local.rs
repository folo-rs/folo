//! Pooled local events that provide automatic resource management for single-threaded use.
//!
//! This module provides pooled variants of local events that automatically manage their lifecycle
//! using reference counting. Events are created from pools and automatically returned to the
//! pool when both sender and receiver are dropped.

#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
use std::cell::RefCell;
use std::future::Future;
use std::marker::PhantomPinned;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::Rc;
use std::task;

use pinned_pool::{Key, PinnedPool};

use crate::{Disconnected, LocalOnceEvent, LocalWithTwoOwners, ReflectiveT, Sealed};

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
    pool: RefCell<PinnedPool<LocalWithTwoOwners<LocalOnceEvent<T>>>>,

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
    #[must_use]
    pub fn bind_by_ref(
        &self,
    ) -> (
        PooledLocalOnceSender<RefLocalPool<'_, T>>,
        PooledLocalOnceReceiver<RefLocalPool<'_, T>>,
    ) {
        let mut inner_pool = self.pool.borrow_mut();
        let inserter = inner_pool.begin_insert();
        let key = inserter.key();

        let item = inserter.insert(LocalWithTwoOwners::new(LocalOnceEvent::new()));

        let item_ptr = NonNull::from(item.get_ref());

        let pool_ref = RefLocalPool { pool: self };

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
    #[must_use]
    pub fn bind_by_rc(
        self: &Rc<Self>,
    ) -> (
        PooledLocalOnceSender<RcLocalPool<T>>,
        PooledLocalOnceReceiver<RcLocalPool<T>>,
    ) {
        let mut inner_pool = self.pool.borrow_mut();
        let inserter = inner_pool.begin_insert();
        let key = inserter.key();

        let item = inserter.insert(LocalWithTwoOwners::new(LocalOnceEvent::new()));

        let item_ptr = NonNull::from(item.get_ref());

        let pool_ref = RcLocalPool {
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
    /// // SAFETY: We ensure the pool is pinned and outlives the sender and receiver
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
        PooledLocalOnceSender<PtrLocalPool<T>>,
        PooledLocalOnceReceiver<PtrLocalPool<T>>,
    ) {
        let mut inner_pool = self.pool.borrow_mut();
        let inserter = inner_pool.begin_insert();
        let key = inserter.key();

        let item = inserter.insert(LocalWithTwoOwners::new(LocalOnceEvent::new()));

        let item_ptr = NonNull::from(item.get_ref());

        let pool_ref = PtrLocalPool {
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

    /// Returns the number of events currently in the pool.
    ///
    /// This represents the count of events that are currently allocated in the pool,
    /// including those that are currently bound to sender/receiver endpoints.
    /// Events are removed from the pool only when both endpoints are dropped.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::LocalOnceEventPool;
    ///
    /// let pool = LocalOnceEventPool::<i32>::new();
    /// assert_eq!(pool.len(), 0);
    ///
    /// let (sender, receiver) = pool.bind_by_ref();
    /// assert_eq!(pool.len(), 1); // Event is in pool while endpoints exist
    ///
    /// drop(sender);
    /// drop(receiver);
    /// assert_eq!(pool.len(), 0); // Event cleaned up after both endpoints dropped
    /// ```
    #[must_use]
    #[inline]
    pub fn len(&self) -> usize {
        self.pool.borrow().len()
    }

    /// Returns whether the pool is empty.
    ///
    /// This is equivalent to `pool.len() == 0` but may be more efficient.
    /// An empty pool may still have reserved capacity.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::LocalOnceEventPool;
    ///
    /// let pool = LocalOnceEventPool::<i32>::new();
    /// assert!(pool.is_empty());
    ///
    /// let (sender, receiver) = pool.bind_by_ref();
    /// assert!(!pool.is_empty()); // Pool has event while endpoints exist
    ///
    /// drop(sender);
    /// drop(receiver);
    /// assert!(pool.is_empty()); // Pool empty after both endpoints dropped
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pool.borrow().is_empty()
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

    /// Uses the provided closure to inspect the backtraces of the current awaiter of each
    /// event in the pool that is currently being awaited by someone.
    ///
    /// This method is only available in debug builds (`cfg(debug_assertions)`).
    /// For any data to be present, `RUST_BACKTRACE=1` or `RUST_LIB_BACKTRACE=1` must be set.
    ///
    /// The closure is called once for each event in the pool that is currently being awaited by
    /// someone.
    #[cfg(debug_assertions)]
    pub fn inspect_awaiters(&self, mut f: impl FnMut(&Backtrace)) {
        let pool = self.pool.borrow();

        for event in pool.iter() {
            event.inspect_awaiter(|bt| {
                if let Some(bt) = bt {
                    f(bt);
                }
            });
        }
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
pub trait LocalPoolRef<T>: Deref<Target = LocalOnceEventPool<T>> + ReflectiveT + Sealed {}

/// An event pool referenced via `&` shared reference.
///
/// Only used in type names. Instances are created internally by [`LocalOnceEventPool`].
#[derive(Copy, Debug)]
pub struct RefLocalPool<'a, T> {
    pool: &'a LocalOnceEventPool<T>,
}

impl<T> Sealed for RefLocalPool<'_, T> {}
impl<T> LocalPoolRef<T> for RefLocalPool<'_, T> {}
impl<T> Deref for RefLocalPool<'_, T> {
    type Target = LocalOnceEventPool<T>;

    fn deref(&self) -> &Self::Target {
        self.pool
    }
}
impl<T> Clone for RefLocalPool<'_, T> {
    fn clone(&self) -> Self {
        Self { pool: self.pool }
    }
}
impl<T> ReflectiveT for RefLocalPool<'_, T> {
    type T = T;
}

/// An event pool referenced via `Rc` shared reference.
///
/// Only used in type names. Instances are created internally by [`LocalOnceEventPool`].
#[derive(Debug)]
pub struct RcLocalPool<T> {
    pool: Rc<LocalOnceEventPool<T>>,
}

impl<T> Sealed for RcLocalPool<T> {}
impl<T> LocalPoolRef<T> for RcLocalPool<T> {}
impl<T> Deref for RcLocalPool<T> {
    type Target = LocalOnceEventPool<T>;

    fn deref(&self) -> &Self::Target {
        &self.pool
    }
}
impl<T> Clone for RcLocalPool<T> {
    fn clone(&self) -> Self {
        Self {
            pool: Rc::clone(&self.pool),
        }
    }
}
impl<T> ReflectiveT for RcLocalPool<T> {
    type T = T;
}

/// An event pool referenced via raw pointer.
///
/// Only used in type names. Instances are created internally by [`LocalOnceEventPool`].
#[derive(Copy, Debug)]
pub struct PtrLocalPool<T> {
    pool: NonNull<LocalOnceEventPool<T>>,
}

impl<T> Sealed for PtrLocalPool<T> {}
impl<T> LocalPoolRef<T> for PtrLocalPool<T> {}
impl<T> Deref for PtrLocalPool<T> {
    type Target = LocalOnceEventPool<T>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The creator of the reference is responsible for ensuring the pool outlives it.
        unsafe { self.pool.as_ref() }
    }
}
impl<T> Clone for PtrLocalPool<T> {
    fn clone(&self) -> Self {
        Self { pool: self.pool }
    }
}
impl<T> ReflectiveT for PtrLocalPool<T> {
    type T = T;
}

/// A sender endpoint for pooled local events that holds a reference to the pool.
///
/// This sender is created from [`LocalOnceEventPool::bind_by_ref`] and automatically manages
/// the lifetime of the underlying event. When both sender and receiver are dropped,
/// the event is automatically returned to the pool.
///
/// This is the single-threaded variant that cannot be sent across threads.
#[derive(Debug)]
pub struct PooledLocalOnceSender<P>
where
    P: LocalPoolRef<<P as ReflectiveT>::T>,
{
    // This is a pointer to avoid contaminating the type signature with the event lifetime.
    //
    // SAFETY: We rely on the inner pool guaranteeing pinning and us owning a counted reference.
    event: Option<NonNull<LocalWithTwoOwners<LocalOnceEvent<P::T>>>>,

    pool_ref: P,
    key: Key,
}

impl<P> PooledLocalOnceSender<P>
where
    P: LocalPoolRef<<P as ReflectiveT>::T>,
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
    #[inline]
    pub fn send(self, value: P::T) {
        // SAFETY: See comments on field.
        let event = unsafe {
            self.event
                .expect("event is only None during destruction")
                .as_ref()
        };

        event.set(value);
    }
}

impl<P> Drop for PooledLocalOnceSender<P>
where
    P: LocalPoolRef<<P as ReflectiveT>::T>,
{
    #[inline]
    fn drop(&mut self) {
        // SAFETY: See comments on field.
        let event = unsafe { self.event.expect("only possible on double drop").as_ref() };

        // The event is going to be destroyed, so we cannot reference it anymore.
        self.event = None;

        // Signal that the sender was dropped before handling reference counting.
        // This ensures receivers get Disconnected errors if the sender is dropped without sending.
        event.sender_dropped();

        if event.release_one() {
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
pub struct PooledLocalOnceReceiver<P>
where
    P: LocalPoolRef<<P as ReflectiveT>::T>,
{
    // This is a pointer to avoid contaminating the type signature with the event lifetime.
    //
    // SAFETY: We rely on the inner pool guaranteeing pinning and us owning a counted reference.
    event: Option<NonNull<LocalWithTwoOwners<LocalOnceEvent<P::T>>>>,

    pool_ref: P,
    key: Key,
}

impl<P> PooledLocalOnceReceiver<P>
where
    P: LocalPoolRef<<P as ReflectiveT>::T>,
{
    /// Drops the inner state, releasing the event back to the pool.
    /// May also be used from contexts where the receiver itself is not yet consumed.
    fn drop_inner(&mut self) {
        let Some(event) = self.event else {
            // Already pseudo-consumed the receiver as part of the Future impl.
            return;
        };

        // Regardless of whether we were the last reference holder or not, we are no longer
        // allowed to reference the event as we are releasing our reference.
        self.event = None;

        // SAFETY: See comments on field.
        let event = unsafe { event.as_ref() };

        if event.release_one() {
            self.pool_ref.pool.borrow_mut().remove(self.key);
        }
    }
}

impl<P> Future for PooledLocalOnceReceiver<P>
where
    P: LocalPoolRef<<P as ReflectiveT>::T>,
{
    type Output = Result<P::T, Disconnected>;

    #[inline]
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

impl<P> Drop for PooledLocalOnceReceiver<P>
where
    P: LocalPoolRef<<P as ReflectiveT>::T>,
{
    #[inline]
    fn drop(&mut self) {
        self.drop_inner();
    }
}

#[cfg(test)]
mod tests {
    use std::pin::pin;
    use std::rc::Rc;

    use futures::task::noop_waker_ref;
    use static_assertions::assert_not_impl_any;
    use testing::with_watchdog;

    use super::*;

    #[test]
    fn local_event_pool_by_ref_basic() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let pool = LocalOnceEventPool::new();

                // Pool starts empty
                assert_eq!(pool.len(), 0);
                assert!(pool.is_empty());

                let (sender, receiver) = pool.bind_by_ref();

                // Pool should have 1 event while endpoints are bound
                assert_eq!(pool.len(), 1);
                assert!(!pool.is_empty());

                sender.send(42);
                let value = receiver.await.unwrap();
                assert_eq!(value, 42);

                // After endpoints are dropped, pool should be empty
                assert_eq!(pool.len(), 0);
                assert!(pool.is_empty());
            });
        });
    }

    #[test]
    fn local_event_pool_by_rc_basic() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let pool = Rc::new(LocalOnceEventPool::new());

                // Pool starts empty
                assert_eq!(pool.len(), 0);
                assert!(pool.is_empty());

                let (sender, receiver) = pool.bind_by_rc();

                // Pool should have 1 event while endpoints are bound
                assert_eq!(pool.len(), 1);
                assert!(!pool.is_empty());

                sender.send(42);
                let value = receiver.await.unwrap();
                assert_eq!(value, 42);

                // After endpoints are dropped, pool should be empty
                assert_eq!(pool.len(), 0);
                assert!(pool.is_empty());
            });
        });
    }

    #[test]
    fn local_event_pool_by_ptr_basic() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let pool = Box::pin(LocalOnceEventPool::new());

                // Pool starts empty
                assert_eq!(pool.len(), 0);
                assert!(pool.is_empty());

                // SAFETY: We ensure the pool is pinned and outlives the sender and receiver
                let (sender, receiver) = unsafe { pool.as_ref().bind_by_ptr() };

                // Pool should have 1 event while endpoints are bound
                assert_eq!(pool.len(), 1);
                assert!(!pool.is_empty());

                sender.send(42);
                let value = receiver.await.unwrap();
                assert_eq!(value, 42);

                // After endpoints are dropped, pool should be empty
                assert_eq!(pool.len(), 0);
                assert!(pool.is_empty());
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

    #[test]
    fn shrink_to_fit_with_empty_pool_shrinks_to_zero() {
        let pool = LocalOnceEventPool::<u32>::new();

        // Create and drop events without using them
        for _ in 0..10 {
            drop(pool.bind_by_ref());
        }

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());

        // Shrink the pool to fit
        pool.shrink_to_fit();

        assert_eq!(
            pool.pool.borrow().capacity(),
            0,
            "Empty pool should shrink to capacity 0"
        );
    }

    #[test]
    fn event_removed_from_pool_after_endpoints_immediate_drop() {
        let pool = LocalOnceEventPool::<u32>::new();

        drop(pool.bind_by_ref());

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn pool_len_and_is_empty_methods() {
        let pool = LocalOnceEventPool::<u32>::new();

        // Initially empty
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());

        // Create first event
        let (sender1, receiver1) = pool.bind_by_ref();
        assert_eq!(pool.len(), 1);
        assert!(!pool.is_empty());

        // Create second event while first is still bound
        let (sender2, receiver2) = pool.bind_by_ref();
        assert_eq!(pool.len(), 2);
        assert!(!pool.is_empty());

        // Drop first event endpoints
        drop(sender1);
        drop(receiver1);
        assert_eq!(pool.len(), 1);
        assert!(!pool.is_empty());

        // Drop second event endpoints
        drop(sender2);
        drop(receiver2);
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn pooled_local_event_receiver_gets_disconnected_when_sender_dropped() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let pool = LocalOnceEventPool::<i32>::new();
                let (sender, receiver) = pool.bind_by_ref();

                // Drop the sender without sending anything
                drop(sender);

                // Receiver should get a Disconnected error
                let result = receiver.await;
                assert!(result.is_err());
                assert!(matches!(result, Err(Disconnected)));
            });
        });
    }

    #[test]
    fn pooled_local_event_by_rc_receiver_gets_disconnected_when_sender_dropped() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let pool = Rc::new(LocalOnceEventPool::<i32>::new());
                let (sender, receiver) = pool.bind_by_rc();

                // Drop the sender without sending anything
                drop(sender);

                // Receiver should get a Disconnected error
                let result = receiver.await;
                assert!(result.is_err());
                assert!(matches!(result, Err(Disconnected)));
            });
        });
    }

    #[test]
    fn pooled_local_event_by_ptr_receiver_gets_disconnected_when_sender_dropped() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let pool = Box::pin(LocalOnceEventPool::<i32>::new());

                // SAFETY: We ensure the pool is pinned and outlives the sender and receiver
                let (sender, receiver) = unsafe { pool.as_ref().bind_by_ptr() };

                // Drop the sender without sending anything
                drop(sender);

                // Receiver should get a Disconnected error
                let result = receiver.await;
                assert!(result.is_err());
                assert!(matches!(result, Err(Disconnected)));
            });
        });
    }

    #[test]
    fn pooled_local_sender_dropped_when_awaiting_signals_disconnected() {
        let pool = LocalOnceEventPool::<i32>::new();
        let (sender, receiver) = pool.bind_by_ref();

        let mut receiver = pin!(receiver);
        let mut context = task::Context::from_waker(noop_waker_ref());
        assert!(matches!(
            receiver.as_mut().poll(&mut context),
            task::Poll::Pending
        ));

        drop(sender);

        let mut context = task::Context::from_waker(noop_waker_ref());
        assert!(matches!(
            receiver.as_mut().poll(&mut context),
            task::Poll::Ready(Err(Disconnected))
        ));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiters_empty_pool() {
        let pool = LocalOnceEventPool::<i32>::new();

        let mut count = 0;
        pool.inspect_awaiters(|_| {
            count += 1;
        });

        assert_eq!(count, 0);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiters_no_awaiters() {
        let pool = LocalOnceEventPool::<String>::new();

        // Create some events but don't await them
        let (_sender1, _receiver1) = pool.bind_by_ref();
        let (_sender2, _receiver2) = pool.bind_by_ref();

        let mut count = 0;
        pool.inspect_awaiters(|_| {
            count += 1;
        });

        assert_eq!(count, 0);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiters_with_awaiters() {
        let pool = LocalOnceEventPool::<i32>::new();

        // Create events and start awaiting them
        let (_sender1, receiver1) = pool.bind_by_ref();
        let (_sender2, receiver2) = pool.bind_by_ref();

        let mut context = task::Context::from_waker(noop_waker_ref());
        let mut pinned_receiver1 = pin!(receiver1);
        let mut pinned_receiver2 = pin!(receiver2);

        // Poll both receivers to create awaiters
        let _poll1 = pinned_receiver1.as_mut().poll(&mut context);
        let _poll2 = pinned_receiver2.as_mut().poll(&mut context);

        let mut count = 0;
        pool.inspect_awaiters(|_backtrace| {
            count += 1;
        });

        assert_eq!(count, 2);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiters_mixed_states() {
        let pool = LocalOnceEventPool::<String>::new();

        // Create multiple events in different states
        let (_sender1, receiver1) = pool.bind_by_ref();
        let (sender2, receiver2) = pool.bind_by_ref();
        let (_sender3, receiver3) = pool.bind_by_ref();

        // Only poll receiver1 and receiver3
        let mut context = task::Context::from_waker(noop_waker_ref());
        let mut pinned_receiver1 = pin!(receiver1);
        let mut pinned_receiver3 = pin!(receiver3);

        let _poll1 = pinned_receiver1.as_mut().poll(&mut context);
        let _poll3 = pinned_receiver3.as_mut().poll(&mut context);

        // Complete sender2 without polling its receiver
        sender2.send("completed".to_string());
        drop(receiver2);

        let mut count = 0;
        pool.inspect_awaiters(|_backtrace| {
            count += 1;
        });

        // Should only count the two that are actually awaiting
        assert_eq!(count, 2);
    }

    #[test]
    fn thread_safety() {
        // Nothing is Send or Sync - everything is stuck on one thread.
        assert_not_impl_any!(LocalOnceEventPool<u32>: Send, Sync);
        assert_not_impl_any!(PooledLocalOnceSender<RefLocalPool<'static, u32>>: Send, Sync);
        assert_not_impl_any!(PooledLocalOnceReceiver<RefLocalPool<'static, u32>>: Send, Sync);
        assert_not_impl_any!(PooledLocalOnceSender<RcLocalPool<u32>>: Send, Sync);
        assert_not_impl_any!(PooledLocalOnceReceiver<RcLocalPool<u32>>: Send, Sync);
        assert_not_impl_any!(PooledLocalOnceSender<PtrLocalPool<u32>>: Send, Sync);
        assert_not_impl_any!(PooledLocalOnceReceiver<PtrLocalPool<u32>>: Send, Sync);
    }
}
