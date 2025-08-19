use std::fmt;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;

use crate::{BlindPool, RawPooled};

/// A mutable reference to a value stored in a [`BlindPool`].
///
/// This type provides automatic lifetime management for values in the pool with exclusive access.
/// When the [`PooledMut`] instance is dropped, the value is automatically removed from the pool.
///
/// Unlike [`Pooled<T>`], this type does not implement [`Clone`] and provides exclusive access
/// through [`DerefMut`], making it suitable for scenarios where mutable access is required
/// and shared ownership is not needed.
///
/// # Thread Safety
///
/// [`PooledMut<T>`] implements thread safety traits conditionally based on the stored type `T`:
///
/// - **Send**: [`PooledMut<T>`] is [`Send`] if and only if `T` is [`Send`]. This allows moving
///   pooled mutable references between threads when the referenced type supports it.
///
/// - **Sync**: [`PooledMut<T>`] is [`Sync`] if and only if `T` is [`Sync`]. This allows sharing
///   the same [`PooledMut<T>`] instance between multiple threads when the referenced type supports
///   concurrent access.
///
/// # Example
///
/// ```rust
/// use blind_pool::BlindPool;
///
/// let pool = BlindPool::new();
/// let mut value_handle = pool.insert_mut("Test".to_string());
///
/// // Mutably access the value.
/// value_handle.push_str(" - Modified");
/// assert_eq!(*value_handle, "Test - Modified");
///
/// // Value is automatically cleaned up when handle is dropped.
/// ```
pub struct PooledMut<T: ?Sized> {
    /// The inner data containing the actual pooled item and pool handle.
    inner: PooledMutInner<T>,
}

/// Internal data structure that manages the lifetime of a mutably pooled item.
struct PooledMutInner<T: ?Sized> {
    /// The typed handle to the actual item in the pool.
    pooled: RawPooled<T>,

    /// A handle to the pool that keeps it alive as long as this item exists.
    pool: BlindPool,
}

impl<T: ?Sized> PooledMut<T> {
    /// Creates a new [`PooledMut<T>`] from a pooled item and pool handle.
    ///
    /// This is an internal constructor used by [`BlindPool::insert_mut`] and
    /// [`BlindPool::insert_with_mut`].
    #[must_use]
    pub(crate) fn new(pooled: RawPooled<T>, pool: BlindPool) -> Self {
        let inner = PooledMutInner { pooled, pool };
        Self { inner }
    }

    /// Provides access to the internal raw pooled handle for type casting operations.
    ///
    /// This method is used internally by the casting macro system and should not be
    /// used directly by user code.
    #[doc(hidden)]
    pub fn __private_cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> PooledMut<U>
    where
        F: FnOnce(&T) -> &U,
    {
        // Cast the RawPooled to the trait object using the provided function
        // SAFETY: The lifetime management logic of this pool guarantees that the target item is
        // still alive in the pool for as long as any handle exists, which it clearly does.
        let cast_pooled = unsafe { self.inner.pooled.__private_cast_dyn_with_fn(cast_fn) };

        PooledMut {
            inner: PooledMutInner {
                pooled: cast_pooled,
                pool: self.inner.pool.clone(),
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
    /// use blind_pool::BlindPool;
    ///
    /// let pool = BlindPool::new();
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
    /// use blind_pool::BlindPool;
    ///
    /// let pool = BlindPool::new();
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
}

impl<T: ?Sized> Deref for PooledMut<T> {
    type Target = T;

    /// Provides direct access to the value stored in the pool.
    ///
    /// This allows the handle to be used as if it were a reference to the stored value.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let pool = BlindPool::new();
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

impl<T: ?Sized> DerefMut for PooledMut<T> {
    /// Provides direct mutable access to the value stored in the pool.
    ///
    /// This allows the handle to be used as if it were a mutable reference to the stored value.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let pool = BlindPool::new();
    /// let mut string_handle = pool.insert_mut("hello".to_string());
    ///
    /// // Mutate the string directly.
    /// string_handle.push_str(" world");
    /// assert_eq!(*string_handle, "hello world");
    /// ```
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: The pooled handle is valid and contains initialized memory of type T.
        // We have exclusive ownership through PooledMut, so no other references can exist.
        unsafe { self.inner.pooled.ptr().as_mut() }
    }
}

impl<T: ?Sized> Drop for PooledMut<T> {
    /// Automatically removes the item from the pool when the handle is dropped.
    ///
    /// This ensures that resources are properly cleaned up without requiring manual intervention.
    #[inline]
    fn drop(&mut self) {
        // We have exclusive ownership, so we can safely remove the item from the pool.
        self.inner.pool.remove(&self.inner.pooled.erase());
    }
}

// SAFETY: PooledMut<T> can be Send if T is Send, because we have exclusive ownership
// of the T and can safely move it between threads if T supports that.
unsafe impl<T: Send> Send for PooledMut<T> {}

// SAFETY: PooledMut<T> can be Sync if T is Sync, because if T is Sync, then shared
// references to PooledMut<T> can safely exist across threads. The mutex in BlindPool
// provides thread safety for pool operations.
unsafe impl<T: Sync> Sync for PooledMut<T> {}

impl<T: ?Sized> fmt::Debug for PooledMut<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PooledMut")
            .field("type_name", &std::any::type_name::<T>())
            .field("ptr", &self.inner.pooled.ptr())
            .finish()
    }
}

impl<T: ?Sized> fmt::Debug for PooledMutInner<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PooledMutInner")
            .field("type_name", &std::any::type_name::<T>())
            .field("ptr", &self.pooled.ptr())
            .field("pool", &self.pool)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::PooledMut;
    use crate::BlindPool;

    #[test]
    fn thread_safety_assertions() {
        // PooledMut<T> should be Send/Sync based on T's Send/Sync status
        assert_impl_all!(PooledMut<u32>: Send, Sync);
        assert_impl_all!(PooledMut<String>: Send, Sync);
        assert_impl_all!(PooledMut<Vec<u8>>: Send, Sync);

        // PooledMut should NOT be Clone
        assert_not_impl_any!(PooledMut<u32>: Clone);
        assert_not_impl_any!(PooledMut<String>: Clone);
        assert_not_impl_any!(PooledMut<Vec<u8>>: Clone);

        // With non-Send/non-Sync types, PooledMut should also not be Send/Sync
        use std::rc::Rc;
        assert_not_impl_any!(PooledMut<Rc<u32>>: Send); // Rc is not Send
        assert_not_impl_any!(PooledMut<Rc<u32>>: Sync); // Rc is not Sync

        // RefCell<T> is Send if T is Send, but not Sync
        use std::cell::RefCell;
        assert_impl_all!(PooledMut<RefCell<u32>>: Send); // RefCell<u32> is Send
        assert_not_impl_any!(PooledMut<RefCell<u32>>: Sync); // RefCell is not Sync
    }

    #[test]
    fn automatic_cleanup() {
        let pool = BlindPool::new();

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
        let pool = BlindPool::new();
        let mut handle = pool.insert_mut("hello".to_string());

        // Test mutable access
        handle.push_str(" world");
        assert_eq!(*handle, "hello world");

        // Test that the modification persists
        assert_eq!(handle.len(), 11);
    }

    #[test]
    fn deref_and_deref_mut_work() {
        let pool = BlindPool::new();
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

        let pool = BlindPool::new();
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
}
