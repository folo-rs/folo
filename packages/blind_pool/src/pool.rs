use std::sync::{Arc, Mutex};

use crate::constants::ERR_POISONED_LOCK;
use crate::{Pooled, RawBlindPool, RawPooled};

/// A thread-safe wrapper around [`RawBlindPool`] that provides automatic resource management
/// and reference counting.
///
/// This is the main pool type that provides automatic resource management through reference
/// counting. For manual resource management, use [`RawBlindPool`] instead.
///
/// This type acts as a cloneable handle to a shared pool instance. Multiple handles
/// can exist simultaneously, and the underlying pool remains alive as long as at least one
/// handle exists.
///
/// Items inserted into the pool are automatically removed when all references to them are
/// dropped, eliminating the need for manual resource management.
///
/// # Thread Safety
///
/// This type is thread-safe and can be safely shared across multiple threads.
///
/// # Example
///
/// ```rust
/// use std::thread;
///
/// use blind_pool::BlindPool;
///
/// let pool = BlindPool::new();
///
/// // Clone the pool handle to share across threads.
/// let pool_clone = pool.clone();
///
/// let handle = thread::spawn(move || {
///     let item_handle = pool_clone.insert(42_u64);
///     *item_handle
/// });
///
/// let value = handle.join().unwrap();
/// assert_eq!(value, 42);
/// ```
#[derive(Clone, Debug)]
pub struct BlindPool {
    /// The shared pool instance protected by a mutex for thread safety.
    inner: Arc<Mutex<RawBlindPool>>,
}

impl BlindPool {
    /// Creates a new [`BlindPool`] with default configuration.
    ///
    /// This is the equivalent of creating a raw pool and wrapping it in management.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let pool = BlindPool::new();
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
        Self {
            inner: Arc::new(Mutex::new(RawBlindPool::new())),
        }
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
    /// use blind_pool::BlindPool;
    ///
    /// let pool = BlindPool::new();
    ///
    /// let u32_handle = pool.insert(42_u32);
    /// let string_handle = pool.insert("hello".to_string());
    ///
    /// // Access values through dereferencing.
    /// assert_eq!(*u32_handle, 42);
    /// assert_eq!(*string_handle, "hello");
    /// ```
    #[inline]
    #[must_use]
    pub fn insert<T>(&self, value: T) -> Pooled<T> {
        let pooled = {
            let mut pool = self.inner.lock().expect(ERR_POISONED_LOCK);
            pool.insert(value)
        };

        Pooled::new(pooled, self.clone())
    }

    /// Returns the total number of items currently stored in the pool.
    ///
    /// This operation may block if another thread is currently accessing the pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let pool = BlindPool::new();
    ///
    /// assert_eq!(pool.len(), 0);
    ///
    /// let _item1 = pool.insert(42_u32);
    /// let _item2 = pool.insert("hello".to_string());
    ///
    /// assert_eq!(pool.len(), 2);
    /// ```
    #[must_use]
    #[inline]
    pub fn len(&self) -> usize {
        let pool = self.inner.lock().expect(ERR_POISONED_LOCK);
        pool.len()
    }

    /// Returns whether the pool has no inserted values.
    ///
    /// This operation may block if another thread is currently accessing the pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let pool = BlindPool::new();
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
    #[inline]
    pub fn is_empty(&self) -> bool {
        let pool = self.inner.lock().expect(ERR_POISONED_LOCK);
        pool.is_empty()
    }

    /// Removes an item from the pool using its handle.
    ///
    /// This is an internal method used by [`Pooled`] when it is dropped.
    #[inline]
    pub(crate) fn remove<T: ?Sized>(&self, pooled: RawPooled<T>) {
        let mut pool = self.inner.lock().expect(ERR_POISONED_LOCK);
        pool.remove(pooled);
    }
}

impl Default for BlindPool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::BlindPool;
    use crate::Pooled;

    #[test]
    fn thread_safety_assertions() {
        // BlindPool should be thread-safe - it can be moved between threads and shared between threads
        assert_impl_all!(BlindPool: Send, Sync);

        // Pooled<T> should be Send if T is Send, because it uses Arc internally
        // and the pool provides thread safety
        assert_impl_all!(Pooled<u32>: Send);
        assert_impl_all!(Pooled<String>: Send);
        assert_impl_all!(Pooled<Vec<u8>>: Send);

        // Pooled<T> should be Sync if T is Sync, because multiple threads can safely
        // access the same Pooled<T> instance when T is Sync
        assert_impl_all!(Pooled<u32>: Sync);
        assert_impl_all!(Pooled<String>: Sync);
        assert_impl_all!(Pooled<Vec<u8>>: Sync);

        // Pooled<T> should NOT be Send/Sync if T is not Send/Sync
        use std::rc::Rc;
        assert_not_impl_any!(Pooled<Rc<u32>>: Send);
        assert_not_impl_any!(Pooled<Rc<u32>>: Sync);

        use std::cell::RefCell;
        assert_impl_all!(Pooled<RefCell<u32>>: Send); // RefCell is Send but not Sync
        assert_not_impl_any!(Pooled<RefCell<u32>>: Sync);
    }

    #[test]
    fn simple_insert_and_access() {
        let pool = BlindPool::new();

        let u32_handle = pool.insert(42_u32);
        let string_handle = pool.insert("hello".to_string());

        // Test dereferencing
        assert_eq!(*u32_handle, 42);
        assert_eq!(*string_handle, "hello");

        // Test len
        assert_eq!(pool.len(), 2);
        assert!(!pool.is_empty());
    }

    #[test]
    fn clone_pool_handles() {
        let pool1 = BlindPool::new();
        let pool2 = pool1.clone();

        // Both pools refer to the same underlying pool
        let value_handle1 = pool1.insert(42_u32);
        assert_eq!(pool2.len(), 1);

        let value_handle2 = pool2.insert(43_u32);
        assert_eq!(pool1.len(), 2);

        // Values are accessible from both pool handles
        assert_eq!(*value_handle1, 42);
        assert_eq!(*value_handle2, 43);
    }

    #[test]
    fn different_types_same_pool() {
        let pool = BlindPool::new();

        let u32_handle = pool.insert(42_u32);
        let f64_handle = pool.insert(2.5_f64);
        let string_handle = pool.insert("test".to_string());

        assert_eq!(pool.len(), 3);

        assert_eq!(*u32_handle, 42);
        assert!(((*f64_handle) - 2.5).abs() < f64::EPSILON);
        assert_eq!(*string_handle, "test");
    }

    #[test]
    fn thread_safety_basic() {
        let pool = BlindPool::new();

        let handle = thread::spawn(move || {
            let item_handle = pool.insert(42_u64);
            *item_handle
        });

        let value = handle.join().unwrap();
        assert_eq!(value, 42);
    }

    #[test]
    fn thread_safety_shared_handles() {
        let pool = BlindPool::new();

        let value_handle = pool.insert(Arc::new(42_u64));
        let cloned_handle = value_handle.clone();

        let handle = thread::spawn(move || **cloned_handle);

        let value = handle.join().unwrap();
        assert_eq!(value, 42);

        // Original handle should still work
        assert_eq!(**value_handle, 42);
    }

    #[test]
    #[cfg(not(miri))] // Miri is too slow when running tests with large data sets
    fn large_number_of_items() {
        let pool = BlindPool::new();

        let mut handles = Vec::new();

        // Insert many items
        for i in 0..1000 {
            handles.push(pool.insert(i));
        }

        assert_eq!(pool.len(), 1000);

        // Verify values
        for (i, handle) in handles.iter().enumerate() {
            assert_eq!(**handle, i);
        }

        // Drop all handles
        handles.clear();
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn auxiliary_functions() {
        let pool = BlindPool::new();

        // Test empty pool
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());

        // Add some items
        let handle1 = pool.insert(42_u32);
        let handle2 = pool.insert("test".to_string());

        assert_eq!(pool.len(), 2);
        assert!(!pool.is_empty());

        // Drop one item
        drop(handle1);
        assert_eq!(pool.len(), 1);
        assert!(!pool.is_empty());

        // Drop remaining item
        drop(handle2);
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }
}
