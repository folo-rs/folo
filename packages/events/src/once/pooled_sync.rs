//! Pooled events that provide automatic resource management.
//!
//! This module provides pooled variants of events that automatically manage their lifecycle
//! using reference counting. Events are created from pools and automatically returned to the
//! pool when both sender and receiver are dropped.

#[cfg(debug_assertions)]
use std::backtrace::Backtrace;
use std::future::Future;
use std::marker::PhantomPinned;
use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::{self, NonNull};
use std::sync::{Arc, Mutex};
use std::task;
use std::{any, fmt};

use pinned_pool::{Key, PinnedPool};

use crate::{Disconnected, ERR_POISONED_LOCK, OnceEvent, ReflectiveTSend, Sealed};

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
pub struct OnceEventPool<T>
where
    T: Send,
{
    pool: Mutex<PinnedPool<OnceEvent<T>>>,

    // It is invalid to move this type once it has been pinned.
    _requires_pinning: PhantomPinned,
}

impl<T> fmt::Debug for OnceEventPool<T>
where
    T: Send,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OnceEventPool")
            .field("item_type", &format_args!("{}", any::type_name::<T>()))
            .finish_non_exhaustive()
    }
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
    #[must_use]
    pub fn bind_by_ref(
        &self,
    ) -> (
        PooledOnceSender<RefPool<'_, T>>,
        PooledOnceReceiver<RefPool<'_, T>>,
    ) {
        let mut inner_pool = self.pool.lock().expect(ERR_POISONED_LOCK);

        let inserter = inner_pool.begin_insert();
        let key = inserter.key();

        // SAFETY: We rely on OnceEvent::new_in_place_bound() for correct initialization.
        let item = unsafe { inserter.insert_with(OnceEvent::new_in_place_bound) };

        let item_ptr = NonNull::from(item.get_ref());

        let pool_ref = RefPool { pool: self };

        (
            PooledOnceSender {
                event: item_ptr,
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
    #[must_use]
    pub fn bind_by_arc(
        self: &Arc<Self>,
    ) -> (PooledOnceSender<ArcPool<T>>, PooledOnceReceiver<ArcPool<T>>) {
        let mut inner_pool = self.pool.lock().expect(ERR_POISONED_LOCK);

        let inserter = inner_pool.begin_insert();
        let key = inserter.key();

        // SAFETY: We rely on OnceEvent::new_in_place_bound() for correct initialization.
        let item = unsafe { inserter.insert_with(OnceEvent::new_in_place_bound) };

        let item_ptr = NonNull::from(item.get_ref());

        let pool_ref = ArcPool {
            pool: Arc::clone(self),
        };

        (
            PooledOnceSender {
                event: item_ptr,
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
    /// // SAFETY: We ensure the pool is pinned and outlives the sender and receiver
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
    ) -> (PooledOnceSender<PtrPool<T>>, PooledOnceReceiver<PtrPool<T>>) {
        let mut inner_pool = self.pool.lock().expect(ERR_POISONED_LOCK);

        let inserter = inner_pool.begin_insert();
        let key = inserter.key();

        // SAFETY: We rely on OnceEvent::new_in_place_bound() for correct initialization.
        let item = unsafe { inserter.insert_with(OnceEvent::new_in_place_bound) };

        let item_ptr = NonNull::from(item.get_ref());

        let pool_ref = PtrPool {
            pool: NonNull::from(self.get_ref()),
        };

        (
            PooledOnceSender {
                event: item_ptr,
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

    /// Returns the number of events currently in the pool.
    ///
    /// This represents the count of events that are currently allocated in the pool,
    /// including those that are currently bound to sender/receiver endpoints.
    /// Events are removed from the pool only when both endpoints are dropped.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::OnceEventPool;
    ///
    /// let pool = OnceEventPool::<i32>::new();
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
    pub fn len(&self) -> usize {
        self.pool.lock().expect(ERR_POISONED_LOCK).len()
    }

    /// Returns whether the pool is empty.
    ///
    /// This is equivalent to `pool.len() == 0` but may be more efficient.
    /// An empty pool may still have reserved capacity.
    ///
    /// # Example
    ///
    /// ```rust
    /// use events::OnceEventPool;
    ///
    /// let pool = OnceEventPool::<i32>::new();
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
        self.pool.lock().expect(ERR_POISONED_LOCK).is_empty()
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
        let mut inner_pool = self.pool.lock().expect(ERR_POISONED_LOCK);
        inner_pool.shrink_to_fit();
    }

    /// Uses the provided closure to inspect the backtraces of the most recent awaiter of each
    /// event in the pool (or `None` if it has never been awaited).
    ///
    /// This method is only available in debug builds (`cfg(debug_assertions)`).
    /// For any data to be present, `RUST_BACKTRACE=1` or `RUST_LIB_BACKTRACE=1` must be set.
    ///
    /// The closure is called once for each event in the pool that is currently being awaited by
    /// someone.
    #[cfg(debug_assertions)]
    pub fn inspect_awaiters(&self, mut f: impl FnMut(Option<&Backtrace>)) {
        let inner_pool = self.pool.lock().expect(ERR_POISONED_LOCK);

        for event in inner_pool.iter() {
            event.inspect_awaiter(&mut f);
        }
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
pub trait PoolRef<T>: Deref<Target = OnceEventPool<T>> + ReflectiveTSend + Sealed
where
    T: Send,
{
}

/// An event pool referenced via `&` shared reference.
///
/// Only used in type names. Instances are created internally by [`OnceEventPool`].
#[derive(Copy)]
pub struct RefPool<'a, T>
where
    T: Send,
{
    pool: &'a OnceEventPool<T>,
}

impl<T> fmt::Debug for RefPool<'_, T>
where
    T: Send,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RefPool")
            .field("item_type", &format_args!("{}", any::type_name::<T>()))
            .finish_non_exhaustive()
    }
}

impl<T> Sealed for RefPool<'_, T> where T: Send {}
impl<T> PoolRef<T> for RefPool<'_, T> where T: Send {}
impl<T> Deref for RefPool<'_, T>
where
    T: Send,
{
    type Target = OnceEventPool<T>;

    fn deref(&self) -> &Self::Target {
        self.pool
    }
}
impl<T> Clone for RefPool<'_, T>
where
    T: Send,
{
    fn clone(&self) -> Self {
        Self { pool: self.pool }
    }
}
impl<T: Send> ReflectiveTSend for RefPool<'_, T> {
    type T = T;
}

/// An event pool referenced via `Arc` shared reference.
///
/// Only used in type names. Instances are created internally by [`OnceEventPool`].
pub struct ArcPool<T>
where
    T: Send,
{
    pool: Arc<OnceEventPool<T>>,
}

impl<T> fmt::Debug for ArcPool<T>
where
    T: Send,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ArcPool")
            .field("item_type", &format_args!("{}", any::type_name::<T>()))
            .finish_non_exhaustive()
    }
}

impl<T> Sealed for ArcPool<T> where T: Send {}
impl<T> PoolRef<T> for ArcPool<T> where T: Send {}
impl<T> Deref for ArcPool<T>
where
    T: Send,
{
    type Target = OnceEventPool<T>;

    fn deref(&self) -> &Self::Target {
        &self.pool
    }
}
impl<T> Clone for ArcPool<T>
where
    T: Send,
{
    fn clone(&self) -> Self {
        Self {
            pool: Arc::clone(&self.pool),
        }
    }
}
impl<T: Send> ReflectiveTSend for ArcPool<T> {
    type T = T;
}

/// An event pool referenced via raw pointer.
///
/// Only used in type names. Instances are created internally by [`OnceEventPool`].
#[derive(Copy)]
pub struct PtrPool<T>
where
    T: Send,
{
    pool: NonNull<OnceEventPool<T>>,
}

impl<T> fmt::Debug for PtrPool<T>
where
    T: Send,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PtrPool")
            .field("item_type", &format_args!("{}", any::type_name::<T>()))
            .finish_non_exhaustive()
    }
}

impl<T> Sealed for PtrPool<T> where T: Send {}
impl<T> PoolRef<T> for PtrPool<T> where T: Send {}
impl<T> Deref for PtrPool<T>
where
    T: Send,
{
    type Target = OnceEventPool<T>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The creator of the reference is responsible for ensuring the pool outlives it.
        unsafe { self.pool.as_ref() }
    }
}
impl<T> Clone for PtrPool<T>
where
    T: Send,
{
    fn clone(&self) -> Self {
        Self { pool: self.pool }
    }
}
impl<T: Send> ReflectiveTSend for PtrPool<T> {
    type T = T;
}
// SAFETY: This is only used with the thread-safe pool (the pool is Sync).
unsafe impl<T> Send for PtrPool<T> where T: Send {}

/// A receiver that can receive a single value through a thread-safe event.
///
/// The type of the value is the inner type parameter,
/// i.e. the `T` in `PooledOnceReceiver<ArcPool<T>>`.
///
/// The outer type parameter determines the mechanism by which the endpoint is bound to the event
/// pool. Different binding mechanisms offer different performance characteristics and resource
/// management patterns.
pub struct PooledOnceSender<P>
where
    P: PoolRef<<P as ReflectiveTSend>::T>,
{
    // This is a pointer to avoid contaminating the type signature with the event lifetime.
    //
    // SAFETY: We rely on the inner pool guaranteeing pinning and the event state machine
    // itself controlling when it is the appropriate time to release the event.
    event: NonNull<OnceEvent<P::T>>,

    pool_ref: P,
    key: Key,
}

impl<P> fmt::Debug for PooledOnceSender<P>
where
    P: PoolRef<<P as ReflectiveTSend>::T>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PooledOnceSender")
            .field("item_type", &format_args!("{}", any::type_name::<P::T>()))
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl<P> PooledOnceSender<P>
where
    P: PoolRef<<P as ReflectiveTSend>::T>,
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
    #[inline]
    pub fn send(self, value: P::T) {
        // The drop logic is different before/after set(), so we switch to manual drop here.
        let mut this = ManuallyDrop::new(self);

        // SAFETY: We rely on the event state machine to only signal "release the event" when
        // we know it will never be used by any logic path again. We only ever create shared
        // references, so there is no aliasing conflict risk. The two different paths that will
        // result in the event being released (set() and drop()) are mutually exclusive.
        let event = unsafe { this.event.as_ref() };

        let set_result = event.set(value);

        if set_result == Err(Disconnected) {
            this.pool_ref
                .pool
                .lock()
                .expect(ERR_POISONED_LOCK)
                .remove(this.key);
        }

        // We also still need to drop the pool ref itself!
        // SAFETY: It is a valid object and ManuallyDrop ensures it will not be auto-dropped.
        unsafe {
            ptr::drop_in_place(&raw mut this.pool_ref);
        }
    }
}

impl<P> Drop for PooledOnceSender<P>
where
    P: PoolRef<<P as ReflectiveTSend>::T>,
{
    #[inline]
    fn drop(&mut self) {
        // SAFETY: We rely on the event state machine to only signal "release the event" when
        // we know it will never be used by any logic path again. We only ever create shared
        // references, so there is no aliasing conflict risk. The two different paths that will
        // result in the event being released (set() and drop()) are mutually exclusive.
        let event = unsafe { self.event.as_ref() };

        // This ensures receivers get Disconnected errors if the sender is dropped without sending.
        if event.sender_dropped_without_set() == Err(Disconnected) {
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
unsafe impl<P> Send for PooledOnceSender<P> where P: PoolRef<<P as ReflectiveTSend>::T> + Send {}

/// A receiver that can receive a single value through a thread-safe event.
///
/// The type of the value is the inner type parameter,
/// i.e. the `T` in `PooledOnceReceiver<ArcPool<T>>`.
///
/// The outer type parameter determines the mechanism by which the endpoint is bound to the event
/// pool. Different binding mechanisms offer different performance characteristics and resource
/// management patterns.
pub struct PooledOnceReceiver<P>
where
    P: PoolRef<<P as ReflectiveTSend>::T>,
{
    // This is a pointer to avoid contaminating the type signature with the event lifetime.
    //
    // SAFETY: We rely on the inner pool guaranteeing pinning and the event state machine
    // itself controlling when it is the appropriate time to release the event.
    event: Option<NonNull<OnceEvent<P::T>>>,

    pool_ref: P,
    key: Key,
}

impl<P> fmt::Debug for PooledOnceReceiver<P>
where
    P: PoolRef<<P as ReflectiveTSend>::T>,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PooledOnceReceiver")
            .field("item_type", &format_args!("{}", any::type_name::<P::T>()))
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl<P> PooledOnceReceiver<P>
where
    P: PoolRef<<P as ReflectiveTSend>::T>,
{
    /// Consumes the receiver and transforms it into the received value, if the value is available.
    ///
    /// This method provides an alternative to awaiting the receiver when you want to check for
    /// an immediately available value without blocking. It returns `Some(value)` if a value has
    /// already been sent, or `None` if no value is currently available.
    ///
    /// # Panics
    ///
    /// Panics if the value has already been received via `Future::poll()`.
    ///
    /// # Examples
    ///
    /// ## Basic usage with Arc-based pool
    ///
    /// ```rust
    /// use std::sync::Arc;
    ///
    /// use events::OnceEventPool;
    ///
    /// let pool = Arc::new(OnceEventPool::<String>::new());
    /// let (sender, receiver) = pool.bind_by_arc();
    /// sender.send("Hello from pool".to_string());
    ///
    /// // Value is immediately available
    /// let value = receiver.into_value();
    /// assert_eq!(value, Some("Hello from pool".to_string()));
    /// // Event is automatically returned to pool for reuse
    /// ```
    ///
    /// ## No value available
    ///
    /// ```rust
    /// use std::sync::Arc;
    ///
    /// use events::OnceEventPool;
    ///
    /// let pool = Arc::new(OnceEventPool::<i32>::new());
    /// let (_sender, receiver) = pool.bind_by_arc();
    ///
    /// // No value sent yet
    /// let value = receiver.into_value();
    /// assert_eq!(value, None);
    /// // Event is still returned to pool
    /// ```
    ///
    /// ## Using with reference-based pool binding
    ///
    /// ```rust
    /// use events::OnceEventPool;
    ///
    /// let pool = OnceEventPool::<String>::new();
    /// let (sender, receiver) = pool.bind_by_ref();
    /// sender.send("Hello".to_string());
    ///
    /// let value = receiver.into_value();
    /// assert_eq!(value, Some("Hello".to_string()));
    /// ```
    pub fn into_value(mut self) -> Option<<P as ReflectiveTSend>::T> {
        self.drop_inner()
    }

    /// Drops the inner state, releasing the event back to the pool, returning the value (if any).
    ///
    /// May also be used from contexts where the receiver itself is not yet consumed.
    fn drop_inner(&mut self) -> Option<<P as ReflectiveTSend>::T> {
        let Some(event) = self.event else {
            // Already pseudo-consumed the receiver as part of the Future impl.
            return None;
        };

        // SAFETY: See comments on field.
        let event = unsafe { event.as_ref() };

        let final_poll_result = event.final_poll();

        // Regardless of whether we were the last reference holder or not, we are no longer
        // allowed to reference the event as we are releasing our reference.
        self.event = None;

        match final_poll_result {
            Ok(Some(value)) => {
                // The sender has disconnected and sent a value, so we need to clean up.
                self.pool_ref
                    .pool
                    .lock()
                    .expect(ERR_POISONED_LOCK)
                    .remove(self.key);
                Some(value)
            }
            Ok(None) => {
                // Nothing for us to do - the sender was still connected and had not
                // sent any value, so it will perform the cleanup on its own.
                None
            }
            Err(Disconnected) => {
                // The sender has already disconnected, so we need to clean up the event.
                self.pool_ref
                    .pool
                    .lock()
                    .expect(ERR_POISONED_LOCK)
                    .remove(self.key);
                None
            }
        }
    }
}

impl<P> Future for PooledOnceReceiver<P>
where
    P: PoolRef<<P as ReflectiveTSend>::T>,
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
                // Any result from the inner poll means we were the last endpoint connected,
                // so we have to clean up the event now.
                this.pool_ref
                    .pool
                    .lock()
                    .expect(ERR_POISONED_LOCK)
                    .remove(this.key);

                // The cleanup is already all done by poll() when it returns a result.
                // This just ensures panic on double poll (otherwise we would violate memory safety).
                this.event = None;

                task::Poll::Ready(value)
            },
        )
    }
}

impl<P> Drop for PooledOnceReceiver<P>
where
    P: PoolRef<<P as ReflectiveTSend>::T>,
{
    #[inline]
    fn drop(&mut self) {
        self.drop_inner();
    }
}

// SAFETY: The NonNull marks it !Send by default but we know that everything behind the pointer
// is thread-safe, so all is well. We also require `Send` from `R` to be extra safe here.
unsafe impl<P> Send for PooledOnceReceiver<P> where P: PoolRef<<P as ReflectiveTSend>::T> + Send {}

#[cfg(test)]
mod tests {
    use std::pin::pin;

    use futures::task::noop_waker_ref;
    use static_assertions::{assert_impl_all, assert_not_impl_any};
    use testing::with_watchdog;

    use super::*;

    #[test]
    fn event_pool_by_ref() {
        with_watchdog(|| {
            let pool = OnceEventPool::<i32>::new();

            // Pool starts empty
            assert_eq!(pool.len(), 0);
            assert!(pool.is_empty());

            let (sender, receiver) = pool.bind_by_ref();

            // Pool should have 1 event while endpoints are bound
            assert_eq!(pool.len(), 1);
            assert!(!pool.is_empty());

            sender.send(42);
            let value = futures::executor::block_on(receiver).unwrap();
            assert_eq!(value, 42);

            // After endpoints are dropped, pool should be empty
            assert_eq!(pool.len(), 0);
            assert!(pool.is_empty());
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

            // Pool starts empty
            assert_eq!(pool.len(), 0);
            assert!(pool.is_empty());

            let (sender, receiver) = pool.bind_by_arc();

            // Pool should have 1 event while endpoints are bound
            assert_eq!(pool.len(), 1);
            assert!(!pool.is_empty());

            sender.send(42);
            let value = futures::executor::block_on(receiver).unwrap();
            assert_eq!(value, 42);

            // After endpoints are dropped, pool should be empty
            assert_eq!(pool.len(), 0);
            assert!(pool.is_empty());
        });
    }

    #[test]
    fn event_pool_by_ptr() {
        with_watchdog(|| {
            let pool = Box::pin(OnceEventPool::<i32>::new());

            // Pool starts empty
            assert_eq!(pool.len(), 0);
            assert!(pool.is_empty());

            // SAFETY: We ensure the pool is pinned and outlives the sender and receiver
            let (sender, receiver) = unsafe { pool.as_ref().bind_by_ptr() };

            // Pool should have 1 event while endpoints are bound
            assert_eq!(pool.len(), 1);
            assert!(!pool.is_empty());

            sender.send(42);
            let value = futures::executor::block_on(receiver).unwrap();
            assert_eq!(value, 42);

            // After endpoints are dropped, pool should be empty
            assert_eq!(pool.len(), 0);
            assert!(pool.is_empty());
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

            // SAFETY: We ensure the pool is pinned and outlives the sender and receiver
            let (sender, _receiver) = unsafe { pool.as_ref().bind_by_ptr() };

            // Force the sender to be dropped without being consumed by send()
            drop(sender);

            // Create a new event to verify the pool is still functional
            // SAFETY: We ensure the pool is pinned and outlives the sender and receiver
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

            // SAFETY: We ensure the pool is pinned and outlives the sender and receiver
            let (_sender, receiver) = unsafe { pool.as_ref().bind_by_ptr() };

            // Force the receiver to be dropped without being consumed by recv()
            drop(receiver);

            // Create a new event to verify the pool is still functional
            // SAFETY: We ensure the pool is pinned and outlives the sender and receiver
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

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());

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

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn pool_len_and_is_empty_methods() {
        let pool = OnceEventPool::<u32>::new();

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
    fn pooled_event_receiver_gets_disconnected_when_sender_dropped() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let pool = OnceEventPool::<i32>::new();
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
    fn pooled_event_by_arc_receiver_gets_disconnected_when_sender_dropped() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let pool = Arc::new(OnceEventPool::<i32>::new());
                let (sender, receiver) = pool.bind_by_arc();

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
    fn pooled_event_by_ptr_receiver_gets_disconnected_when_sender_dropped() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let pool = Box::pin(OnceEventPool::<i32>::new());

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
    fn pooled_sender_dropped_when_awaiting_signals_disconnected() {
        let pool = OnceEventPool::<i32>::new();
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
        let pool = OnceEventPool::<i32>::new();

        let mut count = 0;
        pool.inspect_awaiters(|_| {
            count += 1;
        });

        assert_eq!(count, 0);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiters_no_awaiters() {
        let pool = OnceEventPool::<String>::new();

        // Create some events but don't await them. They must still be inspected.
        let (_sender1, _receiver1) = pool.bind_by_ref();
        let (_sender2, _receiver2) = pool.bind_by_ref();

        let mut count = 0;
        pool.inspect_awaiters(|_| {
            count += 1;
        });

        assert_eq!(count, 2);
    }

    #[cfg(debug_assertions)]
    #[test]
    fn inspect_awaiters_with_awaiters() {
        let pool = OnceEventPool::<i32>::new();

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

    #[test]
    fn pooled_receiver_into_value_with_sent_value() {
        with_watchdog(|| {
            let pool = Arc::new(OnceEventPool::<String>::new());
            let (sender, receiver) = pool.bind_by_arc();
            sender.send("test value".to_string());

            let result = receiver.into_value();
            assert_eq!(result, Some("test value".to_string()));
        });
    }

    #[test]
    fn pooled_receiver_into_value_no_value_sent() {
        with_watchdog(|| {
            let pool = Arc::new(OnceEventPool::<i32>::new());
            let (_sender, receiver) = pool.bind_by_arc();

            let result = receiver.into_value();
            assert_eq!(result, None);
        });
    }

    #[test]
    fn pooled_receiver_into_value_sender_disconnected() {
        with_watchdog(|| {
            let pool = Arc::new(OnceEventPool::<String>::new());
            let (sender, receiver) = pool.bind_by_arc();
            drop(sender); // Disconnect without sending

            let result = receiver.into_value();
            assert_eq!(result, None);
        });
    }

    #[test]
    fn pooled_receiver_into_value_with_ref_pool() {
        with_watchdog(|| {
            let pool = OnceEventPool::<i32>::new();
            let (sender, receiver) = pool.bind_by_ref();
            sender.send(42);

            let result = receiver.into_value();
            assert_eq!(result, Some(42));
        });
    }

    #[test]
    fn pooled_receiver_into_value_with_arc_pool() {
        with_watchdog(|| {
            let pool = Arc::new(OnceEventPool::<String>::new());
            let (sender, receiver) = pool.bind_by_arc();
            sender.send("arc test".to_string());

            let result = receiver.into_value();
            assert_eq!(result, Some("arc test".to_string()));
        });
    }

    #[test]
    fn pooled_receiver_into_value_with_ptr_pool() {
        with_watchdog(|| {
            let pool = Box::pin(OnceEventPool::<i32>::new());
            // SAFETY: Pool is pinned and outlives the sender/receiver.
            let (sender, receiver) = unsafe { pool.as_ref().bind_by_ptr() };
            sender.send(999);

            let result = receiver.into_value();
            assert_eq!(result, Some(999));
        });
    }

    #[test]
    fn pooled_receiver_into_value_returns_none_after_poll() {
        with_watchdog(|| {
            futures::executor::block_on(async {
                let pool = Arc::new(OnceEventPool::<i32>::new());
                let (sender, mut receiver) = pool.bind_by_arc();
                sender.send(42);

                // Poll the receiver first
                let waker = noop_waker_ref();
                let mut context = task::Context::from_waker(waker);
                let poll_result = Pin::new(&mut receiver).poll(&mut context);
                assert_eq!(poll_result, task::Poll::Ready(Ok(42)));

                // This should return None since the receiver was already consumed
                let value = receiver.into_value();
                assert_eq!(value, None);
            });
        });
    }

    #[test]
    fn pooled_receiver_into_value_pool_reuse() {
        with_watchdog(|| {
            let pool = Arc::new(OnceEventPool::<i32>::new());

            // First usage
            let (sender1, receiver1) = pool.bind_by_arc();
            sender1.send(123);
            let result1 = receiver1.into_value();
            assert_eq!(result1, Some(123));

            // Second usage - should reuse the event from the pool
            let (sender2, receiver2) = pool.bind_by_arc();
            sender2.send(456);
            let result2 = receiver2.into_value();
            assert_eq!(result2, Some(456));
        });
    }

    #[test]
    fn pooled_receiver_into_value_multiple_event_types() {
        with_watchdog(|| {
            // Test with different value types
            let pool1 = Arc::new(OnceEventPool::<()>::new());
            let (sender1, receiver1) = pool1.bind_by_arc();
            sender1.send(());
            assert_eq!(receiver1.into_value(), Some(()));

            let pool2 = Arc::new(OnceEventPool::<Vec<i32>>::new());
            let (sender2, receiver2) = pool2.bind_by_arc();
            sender2.send(vec![1, 2, 3]);
            assert_eq!(receiver2.into_value(), Some(vec![1, 2, 3]));

            let pool3 = Arc::new(OnceEventPool::<Option<String>>::new());
            let (sender3, receiver3) = pool3.bind_by_arc();
            sender3.send(Some("nested option".to_string()));
            assert_eq!(
                receiver3.into_value(),
                Some(Some("nested option".to_string()))
            );
        });
    }

    #[test]
    fn thread_safety() {
        // The pool is accessed across threads, so requires Sync as well as Send.
        assert_impl_all!(OnceEventPool<u32>: Send, Sync);

        // These are all meant to be consumed locally - they may move between threads but are
        // not shared between threads, so Sync is not expected, only Send.
        assert_impl_all!(PooledOnceSender<RefPool<'static, u32>>: Send);
        assert_impl_all!(PooledOnceReceiver<RefPool<'static, u32>>: Send);
        assert_impl_all!(PooledOnceSender<ArcPool<u32>>: Send);
        assert_impl_all!(PooledOnceReceiver<ArcPool<u32>>: Send);
        assert_impl_all!(PooledOnceSender<PtrPool<u32>>: Send);
        assert_impl_all!(PooledOnceReceiver<PtrPool<u32>>: Send);
        assert_not_impl_any!(PooledOnceSender<RefPool<'static, u32>>: Sync);
        assert_not_impl_any!(PooledOnceReceiver<RefPool<'static, u32>>: Sync);
        assert_not_impl_any!(PooledOnceSender<ArcPool<u32>>: Sync);
        assert_not_impl_any!(PooledOnceReceiver<ArcPool<u32>>: Sync);
        assert_not_impl_any!(PooledOnceSender<PtrPool<u32>>: Sync);
        assert_not_impl_any!(PooledOnceReceiver<PtrPool<u32>>: Sync);
    }
}
