use std::fmt;
use std::ops::Deref;
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
/// let value_handle = pool.insert(42_u64);
///
/// // Access the value through dereferencing.
/// assert_eq!(*value_handle, 42);
///
/// // Clone to create additional references.
/// let cloned_handle = value_handle.clone();
/// assert_eq!(*cloned_handle, 42);
/// ```
pub struct LocalPooled<T: ?Sized> {
    /// The reference-counted inner data containing the actual pooled item and pool handle.
    inner: Rc<LocalPooledInner<T>>,
}

/// Internal data structure that manages the lifetime of a locally pooled item.
///
/// This is always type-erased to `()` and shared among all typed views of the same item.
/// It ensures that the item is removed from the pool exactly once when all references are dropped.
#[doc(hidden)]
pub struct LocalPooledRef {
    /// The type-erased handle to the actual item in the pool.
    pooled: RawPooled<()>,

    /// A handle to the pool that keeps it alive as long as this item exists.
    pool: LocalBlindPool,
}

/// Internal data structure that contains the typed access to the locally pooled item.
#[non_exhaustive]
pub struct LocalPooledInner<T: ?Sized> {
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
    #[doc(hidden)]
    pub fn new(pooled: RawPooled<T>, pool: LocalBlindPool) -> Self {
        let lifetime = Rc::new(LocalPooledRef {
            pooled: pooled.erase(),
            pool,
        });
        Self { pooled, lifetime }
    }

    /// Creates a new LocalPooledInner sharing the lifetime with an existing one.
    ///
    /// This is used for type casting operations where we want to create a new typed view
    /// while sharing the same lifetime management.
    #[doc(hidden)]
    #[must_use]
    pub fn with_shared_lifetime(pooled: RawPooled<T>, lifetime: Rc<LocalPooledRef>) -> Self {
        Self { pooled, lifetime }
    }
}

impl<T: ?Sized> LocalPooled<T> {
    /// Creates a new [`LocalPooled<T>`] from a pooled item and pool handle.
    ///
    /// This is an internal constructor used by [`LocalBlindPool::insert`] and for
    /// reconstructing pooled values after type casting via the [`define_pooled_dyn_cast!`] macro.
    #[must_use]
    #[doc(hidden)]
    pub fn new(pooled: RawPooled<T>, pool: LocalBlindPool) -> Self {
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
    /// let value_handle = pool.insert(42_u64);
    /// let cloned_handle = value_handle.clone();
    ///
    /// // Erase type information while keeping the original handle via clone.
    /// let erased = value_handle.erase();
    ///
    /// // Both handles are valid and refer to the same item.
    /// assert_eq!(*cloned_handle, 42);
    ///
    /// // The erased handle shares the same reference count.
    /// drop(erased);
    /// assert_eq!(*cloned_handle, 42); // Still accessible via typed handle.
    /// ```
    #[must_use]
    pub fn erase(self) -> LocalPooled<()> {
        // Create a new erased handle sharing the same lifetime manager
        let erased_pooled = self.inner.pooled.erase();
        let erased_inner =
            LocalPooledInner::with_shared_lifetime(erased_pooled, Rc::clone(&self.inner.lifetime));

        LocalPooled {
            inner: Rc::new(erased_inner),
        }
    }

    /// Casts this [`LocalPooled<T>`] to a trait object type.
    ///
    /// This method converts a pooled value from a concrete type to a trait object
    /// while preserving the reference counting and pool management semantics.
    ///
    /// This method is primarily intended for use by the [`define_pooled_dyn_cast!`] macro.
    /// For most use cases, prefer the type-safe cast methods generated by that macro.
    ///
    /// # Panics
    ///
    /// Panics if there are multiple `LocalPooled` handles referring to the same pooled item.
    /// Regular Rust references to the dereferenced value do not count as multiple handles.
    #[must_use]
    /// Casts this [`LocalPooled<T>`] to a trait object type.
    ///
    /// This method converts a pooled value from a concrete type to a trait object
    /// while preserving the reference counting and pool management semantics.
    ///
    /// The returned handle shares the same underlying reference count as the original handle.
    /// Multiple handles (both concrete type and trait object) can coexist for the same pooled item,
    /// and the item will only be removed from the pool when all handles are dropped.
    ///
    /// This method is primarily intended for use by the [`define_pooled_dyn_cast!`] macro.
    /// For most use cases, prefer the type-safe cast methods generated by that macro.
    #[doc(hidden)]
    pub fn cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> LocalPooled<U>
    where
        F: FnOnce(&T) -> &U,
    {
        // Cast the RawPooled to the trait object using the provided function
        let cast_pooled = self.inner.pooled.cast_dyn_with_fn(cast_fn);
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
    /// let value_handle = pool.insert(42_u64);
    ///
    /// let cloned_handle = value_handle.clone();
    ///
    /// // Both handles refer to the same value.
    /// assert_eq!(*value_handle, *cloned_handle);
    ///
    /// // Value remains in pool until all handles are dropped.
    /// drop(value_handle);
    /// assert_eq!(*cloned_handle, 42); // Still accessible.
    /// ```
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
    fn deref(&self) -> &Self::Target {
        // SAFETY: The pooled handle is valid and contains initialized memory of type T.
        // The Rc reference count ensures the underlying pool data remains alive during this access.
        unsafe { self.inner.pooled.ptr().as_ref() }
    }
}

impl Drop for LocalPooledRef {
    /// Automatically removes the item from the pool when the last reference is dropped.
    ///
    /// This ensures that resources are properly cleaned up without requiring manual intervention.
    fn drop(&mut self) {
        // We are guaranteed to be the only one executing this drop because Rc ensures
        // that Drop on the LocalPooledRef is only called once when the last reference is released.
        self.pool.remove(self.pooled);
    }
}

impl<T: fmt::Debug> fmt::Debug for LocalPooled<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalPooled")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<T: fmt::Debug> fmt::Debug for LocalPooledInner<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalPooledInner")
            .field("pooled", &self.pooled)
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

// Explicitly ensure these types are NOT Send or Sync
// by not implementing those traits.
