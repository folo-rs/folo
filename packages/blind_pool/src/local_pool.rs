use std::cell::RefCell;
use std::mem::MaybeUninit;
use std::rc::Rc;

use crate::{LocalPooled, LocalPooledMut, RawBlindPool, RawPooled};

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
/// let item_handle = pool_clone.insert("Test".to_string());
/// assert_eq!(*item_handle, "Test".to_string());
/// ```
#[derive(Clone, Debug)]
pub struct LocalBlindPool {
    /// The shared pool instance protected by a `RefCell` for single-threaded interior mutability.
    inner: Rc<RefCell<RawBlindPool>>,
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
    /// let string1_handle = pool.insert("Test".to_string());
    /// let string2_handle = pool.insert("hello".to_string());
    ///
    /// // Access values through dereferencing.
    /// assert_eq!(*string1_handle, "Test".to_string());
    /// assert_eq!(*string2_handle, "hello");
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: Rc::new(RefCell::new(RawBlindPool::new())),
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
    /// use blind_pool::LocalBlindPool;
    ///
    /// let pool = LocalBlindPool::new();
    ///
    /// let string1_handle = pool.insert("Test".to_string());
    /// let string2_handle = pool.insert("hello".to_string());
    ///
    /// // Access values through dereferencing.
    /// assert_eq!(*string1_handle, "Test".to_string());
    /// assert_eq!(*string2_handle, "hello");
    /// ```
    #[inline]
    #[must_use]
    pub fn insert<T: 'static>(&self, value: T) -> LocalPooled<T> {
        let pooled = {
            let mut pool = self.inner.borrow_mut();
            pool.insert(value)
        };

        LocalPooled::new(pooled, self.clone())
    }

    /// Inserts a value into the pool using in-place initialization and returns a handle to it.
    ///
    /// This method is designed for partial object initialization, where you want to construct
    /// an object directly in its final memory location. This can provide significant
    /// performance benefits compared to [`insert()`] by avoiding temporary allocations
    /// and unnecessary moves, especially for large or complex types.
    ///
    /// [`insert()`]: Self::insert
    ///
    /// The returned handle automatically manages the lifetime of the inserted value.
    /// When all handles to the value are dropped, the value is automatically removed
    /// from the pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::mem::MaybeUninit;
    ///
    /// use blind_pool::LocalBlindPool;
    ///
    /// let pool = LocalBlindPool::new();
    ///
    /// // Partial initialization - build complex object directly in pool memory.
    /// // SAFETY: We properly initialize the value in the closure.
    /// let handle = unsafe {
    ///     pool.insert_with(|uninit: &mut MaybeUninit<Vec<u64>>| {
    ///         let mut vec = Vec::with_capacity(1000);
    ///         vec.extend(0..100);
    ///         uninit.write(vec);
    ///     })
    /// };
    ///
    /// // Access value through dereferencing.
    /// assert_eq!(handle.len(), 100);
    /// ```
    ///
    /// # Safety
    ///
    /// The closure must properly initialize the `MaybeUninit<T>` before returning.
    #[inline]
    #[must_use]
    pub unsafe fn insert_with<T: 'static>(
        &self,
        f: impl FnOnce(&mut MaybeUninit<T>),
    ) -> LocalPooled<T> {
        let pooled = {
            let mut pool = self.inner.borrow_mut();
            // SAFETY: Forwarding safety requirements to caller.
            unsafe { pool.insert_with(f) }
        };

        LocalPooled::new(pooled, self.clone())
    }

    /// Inserts a value into the pool and returns a mutable handle to access it.
    ///
    /// Unlike [`insert()`], this method returns a [`LocalPooledMut<T>`] that provides exclusive
    /// mutable access to the value and does not implement [`Clone`]. This is suitable for
    /// scenarios where you need to modify the value and don't require shared ownership.
    ///
    /// The returned handle automatically manages the lifetime of the inserted value.
    /// When the handle is dropped, the value is automatically removed from the pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::LocalBlindPool;
    ///
    /// let pool = LocalBlindPool::new();
    ///
    /// let mut string_handle = pool.insert_mut("Test".to_string());
    ///
    /// // Mutate the value directly.
    /// string_handle.push_str(" - Modified");
    /// assert_eq!(*string_handle, "Test - Modified");
    /// ```
    ///
    /// [`insert()`]: Self::insert
    #[inline]
    #[must_use]
    pub fn insert_mut<T: 'static>(&self, value: T) -> LocalPooledMut<T> {
        let pooled = {
            let mut pool = self.inner.borrow_mut();
            pool.insert(value)
        };

        LocalPooledMut::new(pooled, self.clone())
    }

    /// Inserts a value into the pool using in-place initialization and returns a mutable handle to it.
    ///
    /// This allows the caller to initialize the item in-place using a closure that receives
    /// a `&mut MaybeUninit<T>`. This can be more efficient than constructing the value
    /// separately and then moving it into the pool, especially for large or complex types.
    ///
    /// Unlike [`insert_with()`], this method returns a [`LocalPooledMut<T>`] that provides exclusive
    /// mutable access to the value and does not implement [`Clone`].
    ///
    /// The returned handle automatically manages the lifetime of the inserted value.
    /// When the handle is dropped, the value is automatically removed from the pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::mem::MaybeUninit;
    ///
    /// use blind_pool::LocalBlindPool;
    ///
    /// let pool = LocalBlindPool::new();
    ///
    /// // SAFETY: We properly initialize the value in the closure.
    /// let mut handle = unsafe {
    ///     pool.insert_with_mut(|uninit: &mut MaybeUninit<String>| {
    ///         uninit.write(String::from("Hello, World!"));
    ///     })
    /// };
    ///
    /// // Mutate the value directly.
    /// handle.push_str(" - Modified");
    /// assert_eq!(*handle, "Hello, World! - Modified");
    /// ```
    ///
    /// # Safety
    ///
    /// The closure must properly initialize the `MaybeUninit<T>` before returning.
    ///
    /// [`insert_with()`]: Self::insert_with
    #[inline]
    #[must_use]
    pub unsafe fn insert_with_mut<T: 'static>(
        &self,
        f: impl FnOnce(&mut MaybeUninit<T>),
    ) -> LocalPooledMut<T> {
        let pooled = {
            let mut pool = self.inner.borrow_mut();
            // SAFETY: Forwarding safety requirements to caller.
            unsafe { pool.insert_with(f) }
        };

        LocalPooledMut::new(pooled, self.clone())
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
    /// let _item1 = pool.insert("Hello".to_string());
    /// let _item2 = pool.insert("hello".to_string());
    ///
    /// assert_eq!(pool.len(), 2);
    /// ```
    #[must_use]
    #[inline]
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
    /// let item = pool.insert("Test".to_string());
    /// assert!(!pool.is_empty());
    ///
    /// drop(item);
    /// assert!(pool.is_empty());
    /// ```
    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        let pool = self.inner.borrow();
        pool.is_empty()
    }

    /// Removes an item from the pool using its handle.
    ///
    /// This is an internal method used by [`LocalPooled`] when it is dropped.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the pooled handle has not been used for removal before.
    /// Using the same pooled handle multiple times may result in undefined behavior.
    #[inline]
    pub(crate) unsafe fn remove<T: ?Sized>(&self, pooled: &RawPooled<T>) {
        let mut pool = self.inner.borrow_mut();
        // SAFETY: The caller guarantees that this pooled handle has not been used before.
        unsafe {
            pool.remove(pooled);
        }
    }

    /// Removes an item from the pool and returns it, without dropping it.
    ///
    /// This is an internal method used by [`LocalPooledMut::into_inner`].
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the pooled handle has not been used for removal before.
    /// Using the same pooled handle multiple times may result in undefined behavior.
    #[inline]
    pub(crate) unsafe fn remove_unpin<T: Unpin>(&self, pooled: &RawPooled<T>) -> T {
        let mut pool = self.inner.borrow_mut();
        // SAFETY: The caller guarantees that this pooled handle has not been used before.
        unsafe { pool.remove_unpin(pooled) }
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

    #[test]
    fn single_threaded_assertions() {
        // LocalBlindPool should NOT be Send or Sync - it's single-threaded only
        assert_not_impl_any!(LocalBlindPool: Send);
        assert_not_impl_any!(LocalBlindPool: Sync);
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

    #[test]
    fn auxiliary_functions() {
        let pool = LocalBlindPool::new();

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

    #[test]
    fn insert_mut_basic_functionality() {
        let pool = LocalBlindPool::new();

        let mut handle = pool.insert_mut("hello".to_string());

        // Test that we can mutate the value
        handle.push_str(" world");
        assert_eq!(*handle, "hello world");

        // Test that pool length is correct
        assert_eq!(pool.len(), 1);

        // Test automatic cleanup
        drop(handle);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn insert_with_mut_basic_functionality() {
        use std::mem::MaybeUninit;

        let pool = LocalBlindPool::new();

        // SAFETY: We properly initialize the String in the closure.
        let mut handle = unsafe {
            pool.insert_with_mut(|uninit: &mut MaybeUninit<String>| {
                uninit.write(String::from("Hello"));
            })
        };

        // Test that we can mutate the value
        handle.push_str(", World!");
        assert_eq!(*handle, "Hello, World!");

        // Test that pool length is correct
        assert_eq!(pool.len(), 1);

        // Test automatic cleanup
        drop(handle);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn insert_mut_different_from_insert() {
        let pool = LocalBlindPool::new();

        // Test that both methods can be used simultaneously
        let _shared_handle = pool.insert("shared".to_string());
        let mut mut_handle = pool.insert_mut("mutable".to_string());

        assert_eq!(pool.len(), 2);

        // Modify only the mutable one
        mut_handle.push_str(" - modified");
        assert_eq!(*mut_handle, "mutable - modified");

        drop(mut_handle);
        assert_eq!(pool.len(), 1); // Only mutable handle dropped
    }
}
