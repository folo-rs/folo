use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::ptr::NonNull;
use std::sync::Arc;

use crate::{BlindPool, RawPooled};

/// A managed handle to a value stored in a [`ManagedBlindPool`].
///
/// This type provides automatic resource management through reference counting. When the last
/// [`ManagedPooled<T>`] referring to a particular value is dropped, the value is automatically
/// removed from the pool.
///
/// The handle also keeps the pool alive - as long as any [`ManagedPooled<T>`] instances exist,
/// the underlying pool will not be deallocated.
///
/// # Thread Safety
///
/// This type inherits the thread safety properties of `T`:
/// - If `T: Send`, then `ManagedPooled<T>: Send`
/// - If `T: Sync`, then `ManagedPooled<T>: Sync`
///
/// # Example
///
/// ```rust
/// use blind_pool::BlindPool;
/// use blind_pool_managed::ManagedBlindPool;
///
/// let pool = ManagedBlindPool::from(BlindPool::new());
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
pub struct Pooled<T> {
    /// The reference-counted inner data containing the actual pooled item and pool handle.
    inner: Arc<PooledInner<T>>,
}

/// Internal data structure that contains the actual pooled item and keeps the pool alive.
struct PooledInner<T> {
    /// The handle to the actual item in the pool.
    pooled: RawPooled<T>,

    /// A handle to the pool that keeps it alive as long as this item exists.
    pool: BlindPool,
}

impl<T> PooledInner<T> {
    /// Extracts the pooled value and pool handle without triggering cleanup.
    ///
    /// This method consumes the inner structure and returns both the pooled value and
    /// pool handle while preventing the Drop implementation from running. This is used
    /// when transferring ownership of the pooled item without removing it from the pool.
    fn into_parts(self) -> (RawPooled<T>, BlindPool) {
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

impl<T> Pooled<T> {
    /// Creates a new [`ManagedPooled<T>`] from a pooled item and pool handle.
    ///
    /// This is an internal constructor used by [`ManagedBlindPool::insert`].
    pub(crate) fn new(pooled: RawPooled<T>, pool: BlindPool) -> Self {
        let inner = PooledInner { pooled, pool };
        Self {
            inner: Arc::new(inner),
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
    /// use blind_pool_managed::ManagedBlindPool;
    ///
    /// let pool = ManagedBlindPool::from(BlindPool::new());
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

    /// Erases the type information from this [`ManagedPooled<T>`] handle,
    /// returning a [`ManagedPooled<()>`].
    ///
    /// This is useful when you want to store handles of different types in the same collection
    /// or pass them to code that doesn't need to know the specific type.
    ///
    /// The handle remains functionally equivalent and will still automatically remove the item
    /// from the pool when dropped. The only change is the removal of the type information.
    ///
    /// # Panics
    ///
    /// Panics if there are multiple `ManagedPooled` handles referring to the same pooled item.
    /// Regular Rust references to the dereferenced value do not count as multiple handles.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    /// use blind_pool_managed::ManagedBlindPool;
    ///
    /// let pool = ManagedBlindPool::from(BlindPool::new());
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
    pub fn erase(self) -> Pooled<()> {
        // We need exclusive access to perform the erase operation.
        // This will panic if there are other references.

        // Move out of self to avoid Drop running
        let this = ManuallyDrop::new(self);

        // SAFETY: We own `this` and ManuallyDrop ensures it will not be auto-dropped.
        let inner_arc = unsafe { std::ptr::read(std::ptr::addr_of!(this.inner)) };

        let inner = Arc::try_unwrap(inner_arc)
            .map_err(|_arc| "cannot erase ManagedPooled with multiple references")
            .unwrap();

        // Extract the pooled value and pool handle without triggering the drop cleanup
        let (pooled, pool) = inner.into_parts();

        let erased_pooled = pooled.erase();

        let erased_inner = PooledInner {
            pooled: erased_pooled,
            pool,
        };
        Pooled {
            inner: Arc::new(erased_inner),
        }
    }
}

impl<T> Clone for Pooled<T> {
    /// Creates another handle to the same pooled value.
    ///
    /// This increases the reference count for the underlying value. The value will only be
    /// removed from the pool when all cloned handles are dropped.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    /// use blind_pool_managed::ManagedBlindPool;
    ///
    /// let pool = ManagedBlindPool::from(BlindPool::new());
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
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> Deref for Pooled<T> {
    type Target = T;

    /// Provides direct access to the value stored in the pool.
    ///
    /// This allows the managed handle to be used as if it were a reference to the stored value.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    /// use blind_pool_managed::ManagedBlindPool;
    ///
    /// let pool = ManagedBlindPool::from(BlindPool::new());
    /// let managed_string = pool.insert("hello".to_string());
    ///
    /// // Access string methods directly.
    /// assert_eq!(managed_string.len(), 5);
    /// assert!(managed_string.starts_with("he"));
    /// ```
    fn deref(&self) -> &Self::Target {
        // SAFETY: The pointer is valid as long as this ManagedPooled exists.
        // The Arc ensures that the underlying data remains alive.
        unsafe { self.inner.pooled.ptr().as_ref() }
    }
}

impl<T> Drop for PooledInner<T> {
    /// Automatically removes the item from the pool when the last reference is dropped.
    ///
    /// This ensures that resources are properly cleaned up without requiring manual intervention.
    fn drop(&mut self) {
        // We are guaranteed to be the only one executing this drop because Arc ensures
        // that Drop on the Inner is only called once when the last reference is released.
        self.pool.remove(self.pooled);
    }
}

// SAFETY: ManagedPooled<T> can be Send if T is Send, because the Arc<ManagedPooledInner<T>>
// is Send when T is Send, and the mutex in ManagedBlindPool provides thread safety.
unsafe impl<T: Send> Send for Pooled<T> {}

// SAFETY: ManagedPooled<T> can be Sync if T is Sync, because multiple threads can safely
// access the same ManagedPooled<T> instance if T is Sync. The deref operation is safe
// for concurrent access when T is Sync, and other operations don't require exclusive access.
unsafe impl<T: Sync> Sync for Pooled<T> {}

impl<T: std::fmt::Debug> std::fmt::Debug for Pooled<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedPooled")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for PooledInner<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ManagedPooledInner")
            .field("pooled", &self.pooled)
            .field("pool", &self.pool)
            .finish()
    }
}

// SAFETY: ManagedPooledInner<T> can be Send if T is Send, following the same reasoning as ManagedPooled<T>.
unsafe impl<T: Send> Send for PooledInner<T> {}

// SAFETY: ManagedPooledInner<T> can be Sync if T is Sync, following the same reasoning as ManagedPooled<T>.
unsafe impl<T: Sync> Sync for PooledInner<T> {}
