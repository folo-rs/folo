use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::ptr::NonNull;
use std::rc::Rc;

use crate::{LocalBlindPool, RawPooled};

/// A single-threaded managed handle to a value stored in a [`LocalManagedBlindPool`].
///
/// This type provides automatic resource management through reference counting. When the last
/// [`LocalManagedPooled<T>`] referring to a particular value is dropped, the value is automatically
/// removed from the pool.
///
/// The handle also keeps the pool alive - as long as any [`LocalManagedPooled<T>`] instances exist,
/// the underlying pool will not be deallocated.
///
/// # Single-threaded Design
///
/// This type is designed for single-threaded use and is neither [`Send`] nor [`Sync`].
/// For multi-threaded scenarios, use [`crate::ManagedPooled`] instead.
///
/// # Example
///
/// ```rust
/// use blind_pool::BlindPool;
/// use blind_pool_managed::LocalManagedBlindPool;
///
/// let pool = LocalManagedBlindPool::from(BlindPool::new());
///
/// let managed_value = pool.insert(42_u64);
///
/// // Access the value through dereferencing.
/// assert_eq!(*managed_value, 42);
///
/// // Get a raw pointer to the value.
/// let ptr = managed_value.ptr();
///
/// // The value is automatically removed when dropped.
/// drop(managed_value);
/// ```
pub struct LocalPooled<T> {
    /// The reference-counted inner data containing the actual pooled item and pool handle.
    inner: Rc<LocalPooledInner<T>>,
}

/// Internal data structure that contains the actual pooled item and keeps the pool alive.
struct LocalPooledInner<T> {
    /// The handle to the actual item in the pool.
    pooled: RawPooled<T>,

    /// A handle to the pool that keeps it alive as long as this item exists.
    pool: LocalBlindPool,
}

impl<T> LocalPooledInner<T> {
    /// Extracts the pooled value and pool handle without triggering cleanup.
    ///
    /// This method consumes the inner structure and returns both the pooled value and
    /// pool handle while preventing the Drop implementation from running. This is used
    /// when transferring ownership of the pooled item without removing it from the pool.
    fn into_parts(self) -> (RawPooled<T>, LocalBlindPool) {
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

impl<T> LocalPooled<T> {
    /// Creates a new [`LocalManagedPooled<T>`] from a pooled item and pool handle.
    ///
    /// This is an internal constructor used by [`LocalManagedBlindPool::insert`].
    pub(crate) fn new(pooled: RawPooled<T>, pool: LocalBlindPool) -> Self {
        let inner = LocalPooledInner { pooled, pool };
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
    /// use blind_pool::BlindPool;
    /// use blind_pool_managed::LocalManagedBlindPool;
    ///
    /// let pool = LocalManagedBlindPool::from(BlindPool::new());
    /// let managed_value = pool.insert(42_u64);
    ///
    /// let ptr = managed_value.ptr();
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

    /// Erases the type information from this [`LocalManagedPooled<T>`] handle,
    /// returning a [`LocalManagedPooled<()>`].
    ///
    /// This is useful when you want to store handles of different types in the same collection
    /// or pass them to code that doesn't need to know the specific type.
    ///
    /// The handle remains functionally equivalent and will still automatically remove the item
    /// from the pool when dropped. The only change is the removal of the type information.
    ///
    /// # Panics
    ///
    /// Panics if there are multiple `LocalManagedPooled` handles referring to the same pooled item.
    /// Regular Rust references to the dereferenced value do not count as multiple handles.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    /// use blind_pool_managed::LocalManagedBlindPool;
    ///
    /// let pool = LocalManagedBlindPool::from(BlindPool::new());
    /// let managed_value = pool.insert(42_u64);
    ///
    /// // Erase type information.
    /// let erased = managed_value.erase();
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
            .map_err(|_rc| "cannot erase LocalManagedPooled with multiple references")
            .unwrap();

        // Extract the pooled value and pool handle without triggering the drop cleanup
        let (pooled, pool) = inner.into_parts();

        let erased_pooled = pooled.erase();

        let erased_inner = LocalPooledInner {
            pooled: erased_pooled,
            pool,
        };
        LocalPooled {
            inner: Rc::new(erased_inner),
        }
    }
}

impl<T> Clone for LocalPooled<T> {
    /// Creates another handle to the same pooled value.
    ///
    /// This increases the reference count for the underlying value. The value will only be
    /// removed from the pool when all cloned handles are dropped.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    /// use blind_pool_managed::LocalManagedBlindPool;
    ///
    /// let pool = LocalManagedBlindPool::from(BlindPool::new());
    /// let managed_value = pool.insert(42_u64);
    ///
    /// let cloned_handle = managed_value.clone();
    ///
    /// // Both handles refer to the same value.
    /// assert_eq!(*managed_value, *cloned_handle);
    ///
    /// // Value remains in pool until all handles are dropped.
    /// drop(managed_value);
    /// assert_eq!(*cloned_handle, 42); // Still accessible.
    /// ```
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
        }
    }
}

impl<T> Deref for LocalPooled<T> {
    type Target = T;

    /// Provides direct access to the value stored in the pool.
    ///
    /// This allows the managed handle to be used as if it were a reference to the stored value.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    /// use blind_pool_managed::LocalManagedBlindPool;
    ///
    /// let pool = LocalManagedBlindPool::from(BlindPool::new());
    /// let managed_string = pool.insert("hello".to_string());
    ///
    /// // Access string methods directly.
    /// assert_eq!(managed_string.len(), 5);
    /// assert!(managed_string.starts_with("he"));
    /// ```
    fn deref(&self) -> &Self::Target {
        // SAFETY: The pointer is valid as long as this LocalManagedPooled exists.
        // The Rc ensures that the underlying data remains alive.
        unsafe { self.inner.pooled.ptr().as_ref() }
    }
}

impl<T> Drop for LocalPooledInner<T> {
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
        f.debug_struct("LocalManagedPooled")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for LocalPooledInner<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalManagedPooledInner")
            .field("pooled", &self.pooled)
            .field("pool", &self.pool)
            .finish()
    }
}

// Explicitly ensure these types are NOT Send or Sync
// by not implementing those traits.
