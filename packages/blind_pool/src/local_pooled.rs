use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::ptr::NonNull;
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

/// Internal data structure that contains the actual pooled item and keeps the pool alive.
#[non_exhaustive]
pub struct LocalPooledInner<T: ?Sized> {
    /// The handle to the actual item in the pool.
    pooled: RawPooled<T>,

    /// A handle to the pool that keeps it alive as long as this item exists.
    pool: LocalBlindPool,
}

impl<T: ?Sized> LocalPooledInner<T> {
    /// Creates a new [`LocalPooledInner<T>`] from a pooled item and pool handle.
    ///
    /// This is an internal constructor used when reconstructing pooled values
    /// after type casting operations via the [`define_pooled_dyn_cast!`] macro.
    #[must_use]
    #[doc(hidden)]
    pub fn new(pooled: RawPooled<T>, pool: LocalBlindPool) -> Self {
        Self { pooled, pool }
    }

    /// Extracts the pooled value and pool handle without triggering cleanup.
    ///
    /// This method consumes the inner structure and returns both the pooled value and
    /// pool handle while preventing the Drop implementation from running. This is used
    /// when transferring ownership of the pooled item without removing it from the pool
    /// during type casting operations via the [`define_pooled_dyn_cast!`] macro.
    #[must_use]
    #[doc(hidden)]
    pub fn into_parts(self) -> (RawPooled<T>, LocalBlindPool) {
        // SAFETY: We own `self` and are about to forget it, preventing Drop from running.
        // This allows us to move the fields out without triggering the destructor.
        let pooled = unsafe { std::ptr::read(std::ptr::addr_of!(self.pooled)) };
        // SAFETY: Same reasoning as above - we own the struct and are preventing Drop.
        let pool = unsafe { std::ptr::read(std::ptr::addr_of!(self.pool)) };

        // Prevent Drop from running, which would remove the item from the pool
        std::mem::forget(self);

        (pooled, pool)
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

    /// Returns a pointer to the inserted value.
    ///
    /// This provides direct access to the value stored in the pool. The caller must ensure
    /// that Rust's aliasing rules are respected when using this pointer.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::LocalBlindPool;
    ///
    /// let pool = LocalBlindPool::new();
    /// let value_handle = pool.insert(42_u64);
    ///
    /// let ptr = value_handle.ptr();
    ///
    /// // SAFETY: The pointer is valid and contains the value we just inserted.
    /// let value = unsafe { ptr.read() };
    /// assert_eq!(value, 42);
    /// ```
    #[must_use]
    #[inline]
    pub fn ptr(&self) -> NonNull<T> {
        self.inner.pooled.ptr()
    }

    /// Erases the type information from this [`LocalPooled<T>`] handle,
    /// returning a [`LocalPooled<()>`].
    ///
    /// This is useful when you want to store handles of different types in the same collection
    /// or pass them to code that doesn't need to know the specific type.
    ///
    /// The handle remains functionally equivalent and will still automatically remove the item
    /// from the pool when dropped. The only change is the removal of the type information.
    ///
    /// # Panics
    ///
    /// Panics if there are multiple `LocalPooled` handles referring to the same pooled item.
    /// Regular Rust references to the dereferenced value do not count as multiple handles.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::LocalBlindPool;
    ///
    /// let pool = LocalBlindPool::new();
    /// let value_handle = pool.insert(42_u64);
    ///
    /// // Erase type information.
    /// let erased = value_handle.erase();
    ///
    /// // Can still access the raw pointer.
    /// // SAFETY: We know this contains a u64.
    /// let value = unsafe { erased.ptr().cast::<u64>().read() };
    /// assert_eq!(value, 42);
    /// ```
    #[must_use]
    pub fn erase(self) -> LocalPooled<()> {
        // We need exclusive access to perform the erase operation.
        // This will panic if there are other references.

        // Move out of self to avoid Drop running
        let this = ManuallyDrop::new(self);

        // SAFETY: We own `this` and ManuallyDrop ensures it will not be auto-dropped.
        let inner_rc = unsafe { std::ptr::read(std::ptr::addr_of!(this.inner)) };

        let inner = Rc::try_unwrap(inner_rc)
            .map_err(|_rc| "cannot erase LocalPooled with multiple references")
            .unwrap();

        // Extract the pooled value and pool handle without triggering the drop cleanup
        let (pooled, pool) = inner.into_parts();

        let erased_pooled = pooled.erase();

        let erased_inner = LocalPooledInner::new(erased_pooled, pool);
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
    #[doc(hidden)]
    pub fn cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> LocalPooled<U>
    where
        F: FnOnce(&T) -> &U,
    {
        // We need exclusive access to perform the cast operation.
        // This will panic if there are other references.

        // Move out of self to avoid Drop running
        let this = ManuallyDrop::new(self);

        // SAFETY: We own `this` and ManuallyDrop ensures it will not be auto-dropped.
        let inner_rc = unsafe { std::ptr::read(std::ptr::addr_of!(this.inner)) };

        let inner = Rc::try_unwrap(inner_rc)
            .map_err(|_rc| "cannot cast LocalPooled with multiple references")
            .unwrap();

        // Extract the pooled value and pool handle without triggering the drop cleanup
        let (pooled, pool) = inner.into_parts();

        // Cast the RawPooled to the trait object using the provided function
        let cast_pooled = pooled.cast_dyn_with_fn(cast_fn);

        let cast_inner = LocalPooledInner::new(cast_pooled, pool);
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
        // SAFETY: The pointer is valid as long as this LocalPooled exists.
        // The Rc ensures that the underlying data remains alive.
        unsafe { self.inner.pooled.ptr().as_ref() }
    }
}

impl<T: ?Sized> Drop for LocalPooledInner<T> {
    /// Automatically removes the item from the pool when the last reference is dropped.
    ///
    /// This ensures that resources are properly cleaned up without requiring manual intervention.
    fn drop(&mut self) {
        // We are guaranteed to be the only one executing this drop because Rc ensures
        // that Drop on the Inner is only called once when the last reference is released.
        self.pool.remove(self.pooled);
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for LocalPooled<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalPooled")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for LocalPooledInner<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalPooledInner")
            .field("pooled", &self.pooled)
            .field("pool", &self.pool)
            .finish()
    }
}

// Explicitly ensure these types are NOT Send or Sync
// by not implementing those traits.
