use std::fmt;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;

use crate::{LocalBlindPool, LocalPooled, RawPooled};

/// A mutable reference to a value stored in a [`LocalBlindPool`].
///
/// This type provides automatic lifetime management for values in the pool with exclusive access.
/// When the [`LocalPooledMut`] instance is dropped, the value is automatically removed from the pool.
///
/// Unlike [`LocalPooled<T>`], this type does not implement [`Clone`] and provides exclusive access
/// through [`DerefMut`], making it suitable for scenarios where mutable access is required
/// and shared ownership is not needed.
///
/// # Single-threaded Design
///
/// This type is designed for single-threaded use and is neither [`Send`] nor [`Sync`].
///
/// # Example
///
/// ```rust
/// use blind_pool::LocalBlindPool;
///
/// let pool = LocalBlindPool::new();
/// let mut value_handle = pool.insert_mut("Test".to_string());
///
/// // Mutably access the value.
/// value_handle.push_str(" - Modified");
/// assert_eq!(*value_handle, "Test - Modified");
///
/// // Value is automatically cleaned up when handle is dropped.
/// ```
pub struct LocalPooledMut<T: ?Sized> {
    /// The inner data containing the actual pooled item and pool handle.
    inner: LocalPooledMutInner<T>,
}

/// Internal data structure that manages the lifetime of a mutably pooled item.
struct LocalPooledMutInner<T: ?Sized> {
    /// The typed handle to the actual item in the pool.
    pooled: RawPooled<T>,

    /// A handle to the pool that keeps it alive as long as this item exists.
    pool: LocalBlindPool,
}

impl<T: ?Sized> LocalPooledMut<T> {
    /// Creates a new [`LocalPooledMut<T>`] from a pooled item and pool handle.
    ///
    /// This is an internal constructor used by [`LocalBlindPool::insert_mut`] and
    /// [`LocalBlindPool::insert_with_mut`].
    #[must_use]
    pub(crate) fn new(pooled: RawPooled<T>, pool: LocalBlindPool) -> Self {
        let inner = LocalPooledMutInner { pooled, pool };
        Self { inner }
    }

    /// Provides access to the internal raw pooled handle for type casting operations.
    ///
    /// This method is used internally by the casting macro system and should not be
    /// used directly by user code.
    #[doc(hidden)]
    pub fn __private_cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> LocalPooledMut<U>
    where
        F: FnOnce(&mut T) -> &mut U,
    {
        // We need to prevent the Drop from running on the original handle while still
        // extracting its fields. This is safe because we're transferring ownership
        // to the new handle.
        let this = std::mem::ManuallyDrop::new(self);
        // SAFETY: We are reading from ManuallyDrop wrapped value to transfer ownership.
        // The field is guaranteed to be initialized and we prevent Drop from running.
        let pooled = unsafe { std::ptr::read(&raw const this.inner.pooled) };
        // SAFETY: We are reading from ManuallyDrop wrapped value to transfer ownership.
        // The field is guaranteed to be initialized and we prevent Drop from running.
        let pool = unsafe { std::ptr::read(&raw const this.inner.pool) };

        // Cast the RawPooled to the trait object using the provided function
        // SAFETY: The lifetime management logic of this pool guarantees that the target item is
        // still alive in the pool for as long as any handle exists, which it clearly does.
        let cast_pooled = unsafe { pooled.__private_cast_dyn_with_fn_mut(cast_fn) };

        // Create the new LocalPooledMut with the cast handle and the same pool
        LocalPooledMut {
            inner: LocalPooledMutInner {
                pooled: cast_pooled,
                pool,
            },
        }
    }

    /// Returns a pinned reference to the value stored in the pool.
    ///
    /// Since values in the pool are always pinned (they never move once inserted),
    /// this method provides safe access to `Pin<&T>` without requiring unsafe code.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::pin::Pin;
    /// use blind_pool::LocalBlindPool;
    ///
    /// let pool = LocalBlindPool::new();
    /// let handle = pool.insert_mut("hello".to_string());
    ///
    /// let pinned: Pin<&String> = handle.as_pin();
    /// assert_eq!(pinned.len(), 5);
    /// ```
    #[must_use]
    #[inline]
    pub fn as_pin(&self) -> Pin<&T> {
        // SAFETY: Values in the pool are always pinned - they never move once inserted.
        // The pool ensures stable addresses for the lifetime of the pooled object.
        unsafe { Pin::new_unchecked(&**self) }
    }

    /// Returns a pinned mutable reference to the value stored in the pool.
    ///
    /// Since values in the pool are always pinned (they never move once inserted),
    /// this method provides safe access to `Pin<&mut T>` without requiring unsafe code.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::pin::Pin;
    /// use blind_pool::LocalBlindPool;
    ///
    /// let pool = LocalBlindPool::new();
    /// let mut handle = pool.insert_mut("hello".to_string());
    ///
    /// let mut pinned: Pin<&mut String> = handle.as_pin_mut();
    /// // Can use Pin methods or deref to &mut String
    /// ```
    #[must_use]
    #[inline]
    pub fn as_pin_mut(&mut self) -> Pin<&mut T> {
        // SAFETY: Values in the pool are always pinned - they never move once inserted.
        // The pool ensures stable addresses for the lifetime of the pooled object.
        // We have exclusive access through &mut self, so this is safe.
        unsafe { Pin::new_unchecked(&mut **self) }
    }

    /// Converts this exclusive [`LocalPooledMut<T>`] handle into a shared [`LocalPooled<T>`] handle.
    ///
    /// This operation consumes the [`LocalPooledMut<T>`] and returns a [`LocalPooled<T>`] that allows
    /// multiple shared references to the same pooled value. Once converted, you can no longer
    /// obtain exclusive (mutable) references to the value, but you can create multiple
    /// shared references through cloning.
    ///
    /// The returned [`LocalPooled<T>`] maintains the same lifetime management semantics -
    /// the value will be automatically removed from the pool when all references
    /// (including the returned [`LocalPooled<T>`]) are dropped.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::LocalBlindPool;
    ///
    /// let pool = LocalBlindPool::new();
    /// let mut exclusive_handle = pool.insert_mut("Test".to_string());
    ///
    /// // Modify the value while we have exclusive access.
    /// exclusive_handle.push_str(" - Modified");
    /// assert_eq!(*exclusive_handle, "Test - Modified");
    ///
    /// // Convert to shared access.
    /// let shared_handle = exclusive_handle.into_shared();
    ///
    /// // Now we can create multiple shared references.
    /// let cloned_handle = shared_handle.clone();
    /// assert_eq!(*shared_handle, "Test - Modified");
    /// assert_eq!(*cloned_handle, "Test - Modified");
    ///
    /// // Value is automatically cleaned up when all handles are dropped.
    /// ```
    #[must_use]
    #[inline]
    pub fn into_shared(self) -> LocalPooled<T> {
        // We need to prevent the Drop from running on the original handle while still
        // extracting its fields. This is safe because we're transferring ownership
        // to the new shared handle.
        let this = std::mem::ManuallyDrop::new(self);
        // SAFETY: We are reading from ManuallyDrop wrapped value to transfer ownership.
        // The fields are guaranteed to be initialized and we prevent Drop from running.
        let pooled = unsafe { std::ptr::read(&raw const this.inner.pooled) };
        // SAFETY: We are reading from ManuallyDrop wrapped value to transfer ownership.
        // The field is guaranteed to be initialized and we prevent Drop from running.
        let pool = unsafe { std::ptr::read(&raw const this.inner.pool) };

        // Create the new shared handle with the same pooled item and pool
        LocalPooled::new(pooled, pool)
    }
}

impl<T: ?Sized> Deref for LocalPooledMut<T> {
    type Target = T;

    /// Provides direct access to the value stored in the pool.
    ///
    /// This allows the handle to be used as if it were a reference to the stored value.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::LocalBlindPool;
    ///
    /// let pool = LocalBlindPool::new();
    /// let string_handle = pool.insert_mut("hello".to_string());
    ///
    /// // Access string methods directly.
    /// assert_eq!(string_handle.len(), 5);
    /// assert!(string_handle.starts_with("he"));
    /// ```
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: The pooled handle is valid and contains initialized memory of type T.
        // The owned inner ensures the underlying pool data remains alive during access.
        unsafe { self.inner.pooled.ptr().as_ref() }
    }
}

impl<T: ?Sized> DerefMut for LocalPooledMut<T> {
    /// Provides direct mutable access to the value stored in the pool.
    ///
    /// This allows the handle to be used as if it were a mutable reference to the stored value.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::LocalBlindPool;
    ///
    /// let pool = LocalBlindPool::new();
    /// let mut string_handle = pool.insert_mut("hello".to_string());
    ///
    /// // Mutate the string directly.
    /// string_handle.push_str(" world");
    /// assert_eq!(*string_handle, "hello world");
    /// ```
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: The pooled handle is valid and contains initialized memory of type T.
        // We have exclusive ownership through LocalPooledMut, so no other references can exist.
        unsafe { self.inner.pooled.ptr().as_mut() }
    }
}

impl<T: ?Sized> Drop for LocalPooledMut<T> {
    /// Automatically removes the item from the pool when the handle is dropped.
    ///
    /// This ensures that resources are properly cleaned up without requiring manual intervention.
    #[inline]
    fn drop(&mut self) {
        // We have exclusive ownership, so we can safely remove the item from the pool.
        self.inner.pool.remove(&self.inner.pooled.erase());
    }
}

// LocalPooledMut<T> implements Unpin because the underlying data is fixed in memory.
// Values in the pool are always pinned and never move once inserted, so the wrapper
// type itself can implement Unpin safely.
impl<T: ?Sized> Unpin for LocalPooledMut<T> {}

impl<T: ?Sized> fmt::Debug for LocalPooledMut<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalPooledMut")
            .field("type_name", &std::any::type_name::<T>())
            .field("ptr", &self.inner.pooled.ptr())
            .finish()
    }
}

impl<T: ?Sized> fmt::Debug for LocalPooledMutInner<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalPooledMutInner")
            .field("type_name", &std::any::type_name::<T>())
            .field("ptr", &self.pooled.ptr())
            .field("pool", &self.pool)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::LocalPooledMut;
    use crate::LocalBlindPool;

    #[test]
    fn single_threaded_assertions() {
        // LocalPooledMut<T> should NOT be Send or Sync regardless of T's Send/Sync status
        assert_not_impl_any!(LocalPooledMut<u32>: Send);
        assert_not_impl_any!(LocalPooledMut<u32>: Sync);
        assert_not_impl_any!(LocalPooledMut<String>: Send);
        assert_not_impl_any!(LocalPooledMut<String>: Sync);
        assert_not_impl_any!(LocalPooledMut<Vec<u8>>: Send);
        assert_not_impl_any!(LocalPooledMut<Vec<u8>>: Sync);

        // LocalPooledMut should NOT be Clone
        assert_not_impl_any!(LocalPooledMut<u32>: Clone);
        assert_not_impl_any!(LocalPooledMut<String>: Clone);
        assert_not_impl_any!(LocalPooledMut<Vec<u8>>: Clone);

        // Even with non-Send/non-Sync types, LocalPooledMut should still not be Send/Sync
        use std::rc::Rc;
        assert_not_impl_any!(LocalPooledMut<Rc<u32>>: Send);
        assert_not_impl_any!(LocalPooledMut<Rc<u32>>: Sync);

        use std::cell::RefCell;
        assert_not_impl_any!(LocalPooledMut<RefCell<u32>>: Send);
        assert_not_impl_any!(LocalPooledMut<RefCell<u32>>: Sync);

        // LocalPooledMut should implement Unpin
        assert_impl_all!(LocalPooledMut<u32>: Unpin);
        assert_impl_all!(LocalPooledMut<String>: Unpin);
        assert_impl_all!(LocalPooledMut<Vec<u8>>: Unpin);
    }

    #[test]
    fn automatic_cleanup() {
        let pool = LocalBlindPool::new();

        {
            let _handle = pool.insert_mut(42_u32);
            assert_eq!(pool.len(), 1);
        }

        // Item should be automatically removed after drop
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn mutable_access() {
        let pool = LocalBlindPool::new();
        let mut handle = pool.insert_mut("hello".to_string());

        // Test mutable access
        handle.push_str(" world");
        assert_eq!(*handle, "hello world");

        // Test that the modification persists
        assert_eq!(handle.len(), 11);
    }

    #[test]
    fn deref_and_deref_mut_work() {
        let pool = LocalBlindPool::new();
        let mut handle = pool.insert_mut(vec![1, 2, 3]);

        // Test Deref
        assert_eq!(handle.len(), 3);
        assert_eq!(*handle.first().expect("vec should not be empty"), 1);

        // Test DerefMut
        handle.push(4);
        *handle.get_mut(0).expect("index 0 should exist") = 10;

        assert_eq!(*handle, vec![10, 2, 3, 4]);
    }

    #[test]
    fn works_with_drop_types() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct DropTracker {
            dropped: Arc<AtomicBool>,
        }

        impl Drop for DropTracker {
            fn drop(&mut self) {
                self.dropped.store(true, Ordering::Relaxed);
            }
        }

        let pool = LocalBlindPool::new();
        let dropped = Arc::new(AtomicBool::new(false));

        {
            let _handle = pool.insert_mut(DropTracker {
                dropped: Arc::clone(&dropped),
            });
            assert!(!dropped.load(Ordering::Relaxed));
        }

        // Item's Drop should have been called when pool handle was dropped
        assert!(dropped.load(Ordering::Relaxed));
    }

    #[test]
    #[allow(
        dead_code,
        reason = "Macro-generated trait only used for casting in this test"
    )]
    fn casting_with_futures() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};

        /// Custom trait for futures returning u32.
        pub(crate) trait MyFuture: Future<Output = u32> {}

        /// Blanket implementation for any Future<Output = u32>.
        impl<T> MyFuture for T where T: Future<Output = u32> {}

        // Generate casting methods for MyFuture.
        crate::define_pooled_dyn_cast!(MyFuture);

        #[allow(
            clippy::unused_async,
            reason = "Need async fn to create Future for testing"
        )]
        async fn echo(val: u32) -> u32 {
            val
        }

        let pool = LocalBlindPool::new();

        // Create the future handle first, then cast it (separate operations)
        let original_handle = pool.insert_mut(echo(10));
        let mut future_handle = original_handle.cast_my_future();

        // After casting, the pool should still have the item
        assert_eq!(pool.len(), 1);

        // Poll the future using the safe pinning method from LocalPooledMut
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);

        // Use the as_pin_mut method to get a properly pinned reference
        let pinned_future = future_handle.as_pin_mut();
        match pinned_future.poll(&mut context) {
            Poll::Ready(result) => {
                assert_eq!(result, 10);
            }
            Poll::Pending => {
                panic!("Simple future should complete immediately");
            }
        }

        assert_eq!(pool.len(), 1); // Should still be 1 after polling

        // Drop should work fine
        drop(future_handle);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn unpin_with_non_unpin_type() {
        use std::marker::PhantomPinned;

        // Create a type that is !Unpin
        struct NotUnpin {
            _pinned: PhantomPinned,
            value: u32,
        }

        // Verify that NotUnpin is indeed !Unpin
        assert_not_impl_any!(NotUnpin: Unpin);

        // LocalPooledMut<NotUnpin> should still be Unpin because the wrapper implements Unpin
        // regardless of T's Unpin status - the pooled data is always pinned in place
        assert_impl_all!(LocalPooledMut<NotUnpin>: Unpin);

        let pool = LocalBlindPool::new();
        let handle = pool.insert_mut(NotUnpin {
            _pinned: PhantomPinned,
            value: 42,
        });

        // Can access the value normally
        assert_eq!(handle.value, 42);
    }

    #[test]
    fn into_shared_conversion() {
        let pool = LocalBlindPool::new();
        let mut exclusive_handle = pool.insert_mut("Test".to_string());

        // Modify the value while we have exclusive access
        exclusive_handle.push_str(" - Modified");
        assert_eq!(*exclusive_handle, "Test - Modified");
        assert_eq!(pool.len(), 1);

        // Convert to shared access
        let shared_handle = exclusive_handle.into_shared();
        assert_eq!(*shared_handle, "Test - Modified");
        assert_eq!(pool.len(), 1); // Should still be 1 item

        // Now we can create multiple shared references
        let cloned_handle = shared_handle.clone();
        assert_eq!(*shared_handle, "Test - Modified");
        assert_eq!(*cloned_handle, "Test - Modified");
        assert_eq!(pool.len(), 1); // Still 1 item

        // Value is automatically cleaned up when all handles are dropped
        drop(shared_handle);
        assert_eq!(pool.len(), 1); // Still alive due to cloned_handle
        assert_eq!(*cloned_handle, "Test - Modified"); // Still accessible

        drop(cloned_handle);
        assert_eq!(pool.len(), 0); // Now cleaned up
    }

    #[test]
    fn into_shared_with_different_types() {
        let pool = LocalBlindPool::new();

        // Test with a Vec
        let mut vec_handle = pool.insert_mut(vec![1, 2, 3]);
        vec_handle.push(4);
        let shared_vec = vec_handle.into_shared();
        assert_eq!(*shared_vec, vec![1, 2, 3, 4]);

        // Test with a u64
        let mut_handle = pool.insert_mut(42_u64);
        let shared_u64 = mut_handle.into_shared();
        assert_eq!(*shared_u64, 42);

        assert_eq!(pool.len(), 2);
        drop(shared_vec);
        drop(shared_u64);
        assert_eq!(pool.len(), 0);
    }
}
