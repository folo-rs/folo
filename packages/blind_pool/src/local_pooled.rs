use std::fmt;
use std::ops::Deref;
use std::pin::Pin;
use std::rc::Rc;

use crate::{LocalBlindPool, RawPooled};

/// A reference to a value stored in a [`LocalBlindPool`].
///
/// This type provides automatic lifetime management for values in the pool.
/// When the last [`LocalPooled`] instance for a value is dropped, the value
/// is automatically removed from the pool.
///
/// Multiple [`LocalPooled`] instances can reference the same value through
/// cloning, implementing reference counting semantics.
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
/// let value_handle = pool.insert("Test".to_string());
///
/// // Access the value through dereferencing.
/// assert_eq!(*value_handle, "Test".to_string());
///
/// // Clone to create additional references.
/// let cloned_handle = value_handle.clone();
/// assert_eq!(*cloned_handle, "Test".to_string());
/// ```
pub struct LocalPooled<T: ?Sized> {
    /// The reference-counted inner data containing the actual pooled item and pool handle.
    inner: Rc<LocalPooledInner<T>>,
}

/// Internal data structure that manages the lifetime of a locally pooled item.
///
/// This is always type-erased to `()` and shared among all typed views of the same item.
/// It ensures that the item is removed from the pool exactly once when all references are dropped.
struct LocalPooledRef {
    /// The type-erased handle to the actual item in the pool.
    pooled: RawPooled<()>,

    /// A handle to the pool that keeps it alive as long as this item exists.
    pool: LocalBlindPool,
}

/// Internal data structure that contains the typed access to the locally pooled item.
#[non_exhaustive]
struct LocalPooledInner<T: ?Sized> {
    /// The typed handle to the actual item in the pool.
    pooled: RawPooled<T>,

    /// A shared reference to the lifetime manager.
    lifetime: Rc<LocalPooledRef>,
}

impl<T: ?Sized> LocalPooledInner<T> {
    /// Creates a new [`LocalPooledInner<T>`] from a pooled item and pool handle.
    ///
    /// This is an internal constructor used when reconstructing pooled values
    /// after type casting operations via the [`define_pooled_dyn_cast!`] macro.
    #[must_use]
    fn new(pooled: RawPooled<T>, pool: LocalBlindPool) -> Self {
        let lifetime = Rc::new(LocalPooledRef {
            pooled: pooled.erase(),
            pool,
        });
        Self { pooled, lifetime }
    }

    /// Creates a new `LocalPooledInner` sharing the lifetime with an existing one.
    ///
    /// This is used for type casting operations where we want to create a new typed view
    /// while sharing the same lifetime management.
    #[must_use]
    fn with_shared_lifetime(pooled: RawPooled<T>, lifetime: Rc<LocalPooledRef>) -> Self {
        Self { pooled, lifetime }
    }
}

impl<T: ?Sized> LocalPooled<T> {
    /// Creates a new [`LocalPooled<T>`] from a pooled item and pool handle.
    ///
    /// This is an internal constructor used by [`LocalBlindPool::insert`] and for
    /// reconstructing pooled values after type casting via the [`define_pooled_dyn_cast!`] macro.
    #[must_use]
    pub(crate) fn new(pooled: RawPooled<T>, pool: LocalBlindPool) -> Self {
        let inner = LocalPooledInner::new(pooled, pool);
        Self {
            inner: Rc::new(inner),
        }
    }

    /// Erases the type information from this [`LocalPooled<T>`] handle,
    /// returning a [`LocalPooled<()>`].
    ///
    /// This is useful when you want to store handles of different types in the same collection
    /// or pass them to code that doesn't need to know the specific type.
    ///
    /// The returned handle shares the same underlying reference count as the original handle.
    /// Multiple handles (both typed and type-erased) can coexist for the same pooled item,
    /// and the item will only be removed from the pool when all handles are dropped.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::LocalBlindPool;
    ///
    /// let pool = LocalBlindPool::new();
    /// let value_handle = pool.insert("Test".to_string());
    /// let cloned_handle = value_handle.clone();
    ///
    /// // Erase type information while keeping the original handle via clone.
    /// let erased = value_handle.erase();
    ///
    /// // Both handles are valid and refer to the same item.
    /// assert_eq!(*cloned_handle, "Test".to_string());
    ///
    /// // The erased handle shares the same reference count.
    /// drop(erased);
    /// assert_eq!(*cloned_handle, "Test".to_string()); // Still accessible via typed handle.
    /// ```
    #[must_use]
    #[inline]
    pub fn erase(self) -> LocalPooled<()> {
        // Create a new erased handle sharing the same lifetime manager
        let erased_pooled = self.inner.pooled.erase();
        let erased_inner =
            LocalPooledInner::with_shared_lifetime(erased_pooled, Rc::clone(&self.inner.lifetime));

        LocalPooled {
            inner: Rc::new(erased_inner),
        }
    }

    /// Returns a pointer to the stored value.
    ///
    /// This provides direct access to the underlying pointer while maintaining the safety
    /// guarantees of the pooled reference. The pointer remains valid as long as any
    /// [`LocalPooled<T>`] handle exists for the same value.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::LocalBlindPool;
    ///
    /// let pool = LocalBlindPool::new();
    /// let value_handle = pool.insert("Test".to_string());
    ///
    /// // Get the pointer to the stored value.
    /// let ptr = value_handle.ptr();
    ///
    /// // SAFETY: The pointer is valid as long as value_handle exists.
    /// let value = unsafe { ptr.as_ref() };
    /// assert_eq!(value, "Test");
    /// ```
    #[must_use]
    #[inline]
    pub fn ptr(&self) -> std::ptr::NonNull<T> {
        self.inner.pooled.ptr()
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
    /// let handle = pool.insert("hello".to_string());
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

    /// Casts this [`LocalPooled<T>`] to a trait object type.
    ///
    /// This method converts a pooled value from a concrete type to a trait object
    /// while preserving the reference counting and pool management semantics.
    ///
    /// This method is only intended for use by the [`define_pooled_dyn_cast!`] macro.
    #[doc(hidden)]
    #[must_use]
    #[inline]
    pub fn __private_cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> LocalPooled<U>
    where
        F: FnOnce(&T) -> &U,
    {
        // Cast the RawPooled to the trait object using the provided function
        // SAFETY: The lifetime management logic of this pool guarantees that the target item is
        // still alive in the pool for as long as any handle exists, which it clearly does.
        // We only ever hand out shared references to the item, so no conflicting `&mut`
        // exclusive references can exist.
        let cast_pooled = unsafe { self.inner.pooled.__private_cast_dyn_with_fn(cast_fn) };
        let cast_inner =
            LocalPooledInner::with_shared_lifetime(cast_pooled, Rc::clone(&self.inner.lifetime));

        LocalPooled {
            inner: Rc::new(cast_inner),
        }
    }
}

impl<T: ?Sized> Clone for LocalPooled<T> {
    /// Creates another handle to the same pooled value.
    ///
    /// This increases the reference count for the underlying value. The value will only be
    /// removed from the pool when all cloned handles are dropped.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::LocalBlindPool;
    ///
    /// let pool = LocalBlindPool::new();
    /// let value_handle = pool.insert("Test".to_string());
    ///
    /// let cloned_handle = value_handle.clone();
    ///
    /// // Both handles refer to the same value.
    /// assert_eq!(*value_handle, *cloned_handle);
    ///
    /// // Value remains in pool until all handles are dropped.
    /// drop(value_handle);
    /// assert_eq!(*cloned_handle, "Test".to_string()); // Still accessible.
    /// ```
    #[inline]
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
        }
    }
}

impl<T: ?Sized> Deref for LocalPooled<T> {
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
    /// let string_handle = pool.insert("hello".to_string());
    ///
    /// // Access string methods directly.
    /// assert_eq!(string_handle.len(), 5);
    /// assert!(string_handle.starts_with("he"));
    /// ```
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: The pooled handle is valid and contains initialized memory of type T.
        // The Rc reference count ensures the underlying pool data remains alive during this access.
        // We only ever hand out shared references, so no exclusive references can exist.
        unsafe { self.inner.pooled.ptr().as_ref() }
    }
}

impl Drop for LocalPooledRef {
    /// Automatically removes the item from the pool when the last reference is dropped.
    ///
    /// This ensures that resources are properly cleaned up without requiring manual intervention.
    #[inline]
    fn drop(&mut self) {
        // We are guaranteed to be the only one executing this drop because Rc ensures
        // that Drop on the LocalPooledRef is only called once when the last reference is released.
        self.pool.remove(&self.pooled);
    }
}

impl<T: ?Sized> fmt::Debug for LocalPooled<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalPooled")
            .field("type_name", &std::any::type_name::<T>())
            .field("ptr", &self.inner.pooled.ptr())
            .finish()
    }
}

impl<T: ?Sized> fmt::Debug for LocalPooledInner<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalPooledInner")
            .field("type_name", &std::any::type_name::<T>())
            .field("ptr", &self.pooled.ptr())
            .field("lifetime", &self.lifetime)
            .finish()
    }
}

impl fmt::Debug for LocalPooledRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalPooledRef")
            .field("pooled", &self.pooled)
            .field("pool", &self.pool)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::LocalPooled;
    use crate::LocalBlindPool;

    #[test]
    fn single_threaded_assertions() {
        // LocalPooled<T> should NOT be Send or Sync regardless of T's Send/Sync status
        // because it uses Rc internally which is not Send/Sync
        assert_not_impl_any!(LocalPooled<u32>: Send);
        assert_not_impl_any!(LocalPooled<u32>: Sync);
        assert_not_impl_any!(LocalPooled<String>: Send);
        assert_not_impl_any!(LocalPooled<String>: Sync);
        assert_not_impl_any!(LocalPooled<Vec<u8>>: Send);
        assert_not_impl_any!(LocalPooled<Vec<u8>>: Sync);

        // Even with non-Send/non-Sync types, LocalPooled should still not be Send/Sync
        use std::rc::Rc;
        assert_not_impl_any!(LocalPooled<Rc<u32>>: Send);
        assert_not_impl_any!(LocalPooled<Rc<u32>>: Sync);

        use std::cell::RefCell;
        assert_not_impl_any!(LocalPooled<RefCell<u32>>: Send);
        assert_not_impl_any!(LocalPooled<RefCell<u32>>: Sync);
    }

    #[test]
    fn automatic_cleanup_single_handle() {
        let pool = LocalBlindPool::new();

        {
            let _u32_handle = pool.insert(42_u32);
            assert_eq!(pool.len(), 1);
        }

        // Item should be automatically removed after drop
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn automatic_cleanup_multiple_handles() {
        let pool = LocalBlindPool::new();

        let u32_handle = pool.insert(42_u32);
        let cloned_handle = u32_handle.clone();

        assert_eq!(pool.len(), 1);

        // Drop first handle - item should remain
        drop(u32_handle);
        assert_eq!(pool.len(), 1);

        // Drop second handle - item should be removed
        drop(cloned_handle);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn clone_handles() {
        let pool = LocalBlindPool::new();

        let value_handle = pool.insert(42_u64);
        let cloned_handle = value_handle.clone();

        // Both handles should refer to the same value
        assert_eq!(*value_handle, 42);
        assert_eq!(*cloned_handle, 42);

        // Modification through one handle should be visible through the other
        // (Note: we can't actually modify since we only have shared references)
        assert_eq!(*value_handle, *cloned_handle);
    }

    #[test]
    fn string_methods_through_deref() {
        let pool = LocalBlindPool::new();

        let string_handle = pool.insert("hello world".to_string());

        // Test that we can call String methods directly
        assert_eq!(string_handle.len(), 11);
        assert!(string_handle.starts_with("hello"));
        assert!(string_handle.ends_with("world"));
        assert!(string_handle.contains("lo wo"));
    }

    #[test]
    fn ptr_access() {
        let pool = LocalBlindPool::new();

        let value_handle = pool.insert(42_u64);

        // Access the value directly through dereferencing
        assert_eq!(*value_handle, 42);

        // Access the value through the ptr() method
        let ptr = value_handle.ptr();
        // SAFETY: The pointer is valid as long as value_handle exists.
        let value = unsafe { ptr.read() };
        assert_eq!(value, 42);
    }

    #[test]
    fn erase_type_information() {
        let pool = LocalBlindPool::new();

        let u64_handle = pool.insert(42_u64);
        let typed_clone = u64_handle.clone();
        let erased = u64_handle.erase();

        // Verify the typed handle still works
        assert_eq!(*typed_clone, 42);

        // Pool should still contain the item
        assert_eq!(pool.len(), 1);

        drop(erased);
        drop(typed_clone);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn erase_with_multiple_references_works() {
        let pool = LocalBlindPool::new();

        let value_handle = pool.insert(42_u64);
        let cloned_handle = value_handle.clone();

        // This should now work without panicking
        let erased = value_handle.erase();

        // Both handles should still work
        assert_eq!(*cloned_handle, 42);

        // Verify the erased handle is valid by ensuring cleanup works properly
        drop(erased);
        assert_eq!(*cloned_handle, 42); // Typed handle should still work

        drop(cloned_handle);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn drop_with_types_that_have_drop() {
        let pool = LocalBlindPool::new();

        // Vec has a non-trivial Drop implementation
        let vec_handle = pool.insert(vec![1, 2, 3, 4, 5]);

        assert_eq!(vec_handle.len(), 5);
        assert_eq!(pool.len(), 1);

        drop(vec_handle);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn works_with_single_byte_type() {
        let pool = LocalBlindPool::new();

        let u8_handle = pool.insert(255_u8);

        assert_eq!(*u8_handle, 255);
        assert_eq!(pool.len(), 1);

        drop(u8_handle);
        assert_eq!(pool.len(), 0);
    }
}
