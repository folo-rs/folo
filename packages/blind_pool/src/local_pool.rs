use std::cell::RefCell;
use std::rc::Rc;

use crate::{LocalBlindPoolBuilder, LocalPooled, RawBlindPool, RawPooled};

/// A single-threaded wrapper around [`RawBlindPool`] that provides automatic resource management
/// and reference counting.
///
/// This type acts as a cloneable handle to a shared [`RawBlindPool`] instance. Multiple handles
/// can exist simultaneously, and the underlying pool remains alive as long as at least one
/// handle exists.
///
/// Items inserted into the pool are automatically removed when all references to them are
/// dropped, eliminating the need for manual resource management.
///
/// # Single-threaded Design
///
/// This type is designed for single-threaded use and is neither [`Send`] nor [`Sync`].
/// For multi-threaded scenarios, use [`crate::BlindPool`] instead.
///
/// # Example
///
/// ```rust
/// use blind_pool::LocalBlindPool;
///
/// let pool = LocalBlindPool::new();
///
/// // Clone the pool handle for use in different parts of the code.
/// let pool_clone = pool.clone();
///
/// let item_handle = pool_clone.insert(42_u64);
/// assert_eq!(*item_handle, 42);
/// ```
#[derive(Clone, Debug)]
pub struct LocalBlindPool {
    /// The shared pool instance protected by a `RefCell` for single-threaded interior mutability.
    inner: Rc<RefCell<RawBlindPool>>,
}

impl From<RawBlindPool> for LocalBlindPool {
    /// Creates a new [`LocalBlindPool`] from an existing raw pool.
    ///
    /// The provided pool is consumed and wrapped in single-threaded reference counting.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::{DropPolicy, LocalBlindPool, RawBlindPool};
    ///
    /// // Create a configured raw pool.
    /// let raw_pool = RawBlindPool::builder()
    ///     .drop_policy(DropPolicy::MayDropItems)
    ///     .build();
    ///
    /// // Convert to local pool.
    /// let pool = LocalBlindPool::from(raw_pool);
    /// ```
    fn from(pool: RawBlindPool) -> Self {
        Self {
            inner: Rc::new(RefCell::new(pool)),
        }
    }
}

impl LocalBlindPool {
    /// Creates a new [`LocalBlindPool`] with default configuration.
    ///
    /// This is the equivalent of creating a raw pool and wrapping it in single-threaded management.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::LocalBlindPool;
    ///
    /// let pool = LocalBlindPool::new();
    ///
    /// let u32_handle = pool.insert(42_u32);
    /// let string_handle = pool.insert("hello".to_string());
    ///
    /// // Access values through dereferencing.
    /// assert_eq!(*u32_handle, 42);
    /// assert_eq!(*string_handle, "hello");
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::from(RawBlindPool::new())
    }

    /// Returns a builder for creating a [`LocalBlindPool`] with custom configuration.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::{DropPolicy, LocalBlindPool};
    ///
    /// let pool = LocalBlindPool::builder()
    ///     .drop_policy(DropPolicy::MustNotDropItems)
    ///     .build();
    /// ```
    pub fn builder() -> LocalBlindPoolBuilder {
        LocalBlindPoolBuilder::new()
    }

    /// Inserts a value into the pool and returns a handle to access it.
    ///
    /// The returned handle automatically manages the lifetime of the inserted value.
    /// When all handles to the value are dropped, the value is automatically removed
    /// from the pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::LocalBlindPool;
    ///
    /// let pool = LocalBlindPool::new();
    ///
    /// let u32_handle = pool.insert(42_u32);
    /// let string_handle = pool.insert("hello".to_string());
    ///
    /// // Access values through dereferencing.
    /// assert_eq!(*u32_handle, 42);
    /// assert_eq!(*string_handle, "hello");
    /// ```
    pub fn insert<T>(&self, value: T) -> LocalPooled<T> {
        let pooled = {
            let mut pool = self.inner.borrow_mut();
            pool.insert(value)
        };

        LocalPooled::new(pooled, self.clone())
    }

    /// Returns the total number of items currently stored in the pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::LocalBlindPool;
    ///
    /// let pool = LocalBlindPool::new();
    ///
    /// assert_eq!(pool.len(), 0);
    ///
    /// let _item1 = pool.insert(42_u32);
    /// let _item2 = pool.insert("hello".to_string());
    ///
    /// assert_eq!(pool.len(), 2);
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        let pool = self.inner.borrow();
        pool.len()
    }

    /// Returns whether the pool has no inserted values.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::LocalBlindPool;
    ///
    /// let pool = LocalBlindPool::new();
    ///
    /// assert!(pool.is_empty());
    ///
    /// let item = pool.insert(42_u32);
    /// assert!(!pool.is_empty());
    ///
    /// drop(item);
    /// assert!(pool.is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        let pool = self.inner.borrow();
        pool.is_empty()
    }

    /// Removes an item from the pool using its handle.
    ///
    /// This is an internal method used by [`LocalPooled`] when it is dropped.
    /// It should not be called directly by user code.
    pub(crate) fn remove<T: ?Sized>(&self, pooled: RawPooled<T>) {
        let mut pool = self.inner.borrow_mut();
        pool.remove(pooled);
    }
}

impl Default for LocalBlindPool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::LocalBlindPool;
    use crate::LocalBlindPoolBuilder;

    #[test]
    fn single_threaded_assertions() {
        // LocalBlindPool should NOT be Send or Sync - it's single-threaded only
        assert_not_impl_any!(LocalBlindPool: Send);
        assert_not_impl_any!(LocalBlindPool: Sync);

        // LocalBlindPoolBuilder should be thread-mobile (Send) but not thread-safe (Sync),
        // even though the pool it creates is single-threaded
        use static_assertions::assert_impl_all;
        assert_impl_all!(LocalBlindPoolBuilder: Send);
        assert_not_impl_any!(LocalBlindPoolBuilder: Sync);
    }

    #[test]
    fn simple_insert_and_access() {
        let pool = LocalBlindPool::new();

        let u32_handle = pool.insert(42_u32);
        let string_handle = pool.insert("hello".to_string());

        assert_eq!(*u32_handle, 42);
        assert_eq!(*string_handle, "hello");
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn clone_pool_handles() {
        let pool = LocalBlindPool::new();
        let pool_clone = pool.clone();

        let u32_handle = pool.insert(42_u32);
        let string_handle = pool_clone.insert("test".to_string());

        assert_eq!(*u32_handle, 42);
        assert_eq!(*string_handle, "test");
        assert_eq!(pool.len(), 2);
        assert_eq!(pool_clone.len(), 2); // Should be the same pool
    }

    #[test]
    fn different_types_same_pool() {
        let pool = LocalBlindPool::new();

        let u32_handle = pool.insert(42_u32);
        let f64_handle = pool.insert(2.5_f64);
        let string_handle = pool.insert("test".to_string());

        assert_eq!(pool.len(), 3);

        assert_eq!(*u32_handle, 42);
        assert!(((*f64_handle) - 2.5).abs() < f64::EPSILON);
        assert_eq!(*string_handle, "test");
    }

    #[test]
    #[cfg(not(miri))] // Miri is too slow when running tests with large data sets
    fn large_number_of_items() {
        let pool = LocalBlindPool::new();

        let mut handles = Vec::new();

        // Insert 1000 items
        for i in 0..1000 {
            let handle = pool.insert(i);
            handles.push(handle);
        }

        assert_eq!(pool.len(), 1000);

        // Verify all values are correct
        for (i, handle) in handles.iter().enumerate() {
            assert_eq!(**handle, i);
        }

        // Drop all handles
        drop(handles);

        // Pool should be empty
        assert_eq!(pool.len(), 0);
    }
}
