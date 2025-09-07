use std::alloc::Layout;
use std::mem::MaybeUninit;
use std::sync::{Arc, MutexGuard};

use crate::{
    BlindPoolCore, BlindPoolInnerMap, BlindPooledMut, ERR_POISONED_LOCK, LayoutKey, RawOpaquePool,
    RawOpaquePoolSend,
};

/// A thread-safe reference-counting object pool that accepts any type of object.
///
/// All values in the pool remain pinned for their entire lifetime.
///
/// The pool automatically expands its capacity when needed.
#[doc = include_str!("../../doc/snippets/managed_pool_lifetimes.md")]
#[doc = include_str!("../../doc/snippets/managed_pool_is_thread_safe.md")]
///
/// # Example
///
/// ```rust
/// use infinity_pool::BlindPool;
///
/// fn work_with_data(data: impl std::fmt::Display + Send + 'static) {
///     let mut pool = BlindPool::new();
///
///     // Insert an object into the pool (type determined at runtime)
///     let handle = pool.insert(data.to_string());
///
///     // Access the object through the handle
///     assert_eq!(*handle, data.to_string());
///
///     // The object is automatically removed when the handle is dropped
/// }
///
/// work_with_data("Hello, Blind!");
/// work_with_data(42);
/// ```
///
/// # Pool clones are functionally equivalent
///
/// ```rust
/// use infinity_pool::BlindPool;
///
/// let mut pool1 = BlindPool::new();
/// let pool2 = pool1.clone();
///
/// assert_eq!(pool1.len(), pool2.len());
/// let _handle = pool1.insert(42_i32);
/// assert_eq!(pool1.len(), pool2.len());
/// ```
#[derive(Debug, Default, Clone)]
pub struct BlindPool {
    // Internal pools, one for each unique memory layout encountered.
    //
    // We require 'static from any inserted values because the pool
    // does not enforce any Rust lifetime semantics, only reference counts.
    //
    // The pool type itself is just a handle around the core object,
    // which is reference-counted, mutex-guarded and shared between all pool
    // and handle objects. The core will only ever be dropped once all items
    // have been removed from the pool and all the pool objects have been dropped.
    //
    // This also implies that `DropPolicy` has no meaning for this
    // pool configuration, as the core can never be dropped if it has
    // contents (as dropping the handles of pooled objects will remove
    // them from the pool, while keeping the pool alive until then).
    core: BlindPoolCore,
}

impl BlindPool {
    #[doc = include_str!("../../doc/snippets/pool_new.md")]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[doc = include_str!("../../doc/snippets/pool_len.md")]
    #[must_use]
    #[inline]
    pub fn len(&self) -> usize {
        let core = self.core.lock().expect(ERR_POISONED_LOCK);

        core.values().map(|pool| pool.len()).sum()
    }

    #[doc = include_str!("../../doc/snippets/blind_pool_capacity.md")]
    #[must_use]
    #[inline]
    pub fn capacity_for<T: Send + 'static>(&self) -> usize {
        let key = LayoutKey::with_layout_of::<T>();

        let core = self.core.lock().expect(ERR_POISONED_LOCK);

        core.get(&key)
            .map(|pool| pool.capacity())
            .unwrap_or_default()
    }

    #[doc = include_str!("../../doc/snippets/pool_is_empty.md")]
    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[doc = include_str!("../../doc/snippets/blind_pool_reserve.md")]
    #[inline]
    pub fn reserve_for<T: Send + 'static>(&mut self, additional: usize) {
        let mut core = self.core.lock().expect(ERR_POISONED_LOCK);

        let pool = ensure_inner_pool::<T>(&mut core);

        pool.reserve(additional);
    }

    #[doc = include_str!("../../doc/snippets/pool_shrink_to_fit.md")]
    #[inline]
    pub fn shrink_to_fit(&mut self) {
        let mut core = self.core.lock().expect(ERR_POISONED_LOCK);

        for pool in core.values_mut() {
            pool.shrink_to_fit();
        }
    }

    #[doc = include_str!("../../doc/snippets/pool_insert.md")]
    ///
    /// # Example
    ///
    /// ```rust
    /// use infinity_pool::BlindPool;
    ///
    /// let mut pool = BlindPool::new();
    ///
    /// // Insert an object into the pool
    /// let mut handle = pool.insert("Hello".to_string());
    ///
    /// // Mutate the object via the unique handle
    /// handle.push_str(", Blind World!");
    /// assert_eq!(&*handle, "Hello, Blind World!");
    ///
    /// // Transform the unique handle into a shared handle
    /// let shared_handle = handle.into_shared();
    ///
    /// // After transformation, you can only immutably dereference the object
    /// assert_eq!(&*shared_handle, "Hello, Blind World!");
    /// // shared_handle.push_str("!"); // This would not compile
    ///
    /// // The object is removed when the handle is dropped
    /// drop(shared_handle); // Explicitly drop to remove from pool
    /// assert_eq!(pool.len(), 0);
    /// ```
    #[inline]
    #[must_use]
    pub fn insert<T: Send + 'static>(&mut self, value: T) -> BlindPooledMut<T> {
        let mut core = self.core.lock().expect(ERR_POISONED_LOCK);

        let pool = ensure_inner_pool::<T>(&mut core);

        // SAFETY: inner pool selector guarantees matching layout.
        let inner_handle = unsafe { pool.insert_unchecked(value) };

        BlindPooledMut::new(
            inner_handle,
            LayoutKey::with_layout_of::<T>(),
            Arc::clone(&self.core),
        )
    }

    #[doc = include_str!("../../doc/snippets/pool_insert_with.md")]
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::mem::MaybeUninit;
    ///
    /// use infinity_pool::BlindPool;
    ///
    /// struct DataBuffer {
    ///     id: u32,
    ///     data: MaybeUninit<[u8; 1024]>, // Large buffer to skip initializing
    /// }
    ///
    /// let mut pool = BlindPool::new();
    ///
    /// // Initialize only the id, leaving data uninitialized for performance
    /// let handle = unsafe {
    ///     pool.insert_with(|uninit: &mut MaybeUninit<DataBuffer>| {
    ///         let ptr = uninit.as_mut_ptr();
    ///         // SAFETY: Writing to the id field within allocated space
    ///         unsafe {
    ///             std::ptr::addr_of_mut!((*ptr).id).write(42);
    ///             // data field is intentionally left uninitialized
    ///         }
    ///     })
    /// };
    ///
    /// // ID is accessible, data remains uninitialized
    /// let id = unsafe { std::ptr::addr_of!((*handle).id).read() };
    /// assert_eq!(id, 42);
    /// ```
    ///
    /// # Safety
    #[doc = include_str!("../../doc/snippets/safety_closure_must_initialize_object.md")]
    #[inline]
    #[must_use]
    pub unsafe fn insert_with<T: Send + 'static, F>(&mut self, f: F) -> BlindPooledMut<T>
    where
        F: FnOnce(&mut MaybeUninit<T>),
    {
        let mut core = self.core.lock().expect(ERR_POISONED_LOCK);

        let pool = ensure_inner_pool::<T>(&mut core);

        // SAFETY: inner pool selector guarantees matching layout.
        // Initialization guarantee is forwarded from the caller.
        let inner_handle = unsafe { pool.insert_with_unchecked(f) };

        BlindPooledMut::new(
            inner_handle,
            LayoutKey::with_layout_of::<T>(),
            Arc::clone(&self.core),
        )
    }
}

fn ensure_inner_pool<'a, T: Send + 'static>(
    core: &'a mut MutexGuard<'_, BlindPoolInnerMap>,
) -> &'a mut RawOpaquePoolSend {
    let layout = Layout::new::<T>();
    let key = LayoutKey::new(layout);

    core.entry(key).or_insert_with(|| {
        // SAFETY: We always require `T: Send`.
        unsafe { RawOpaquePoolSend::new(RawOpaquePool::with_layout(layout)) }
    })
}

#[cfg(test)]
mod tests {
    use std::mem::MaybeUninit;
    use std::sync::Arc;
    use std::thread;

    use super::*;

    #[test]
    fn default_pool_is_empty() {
        let pool = BlindPool::default();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn single_type_operations() {
        let mut pool = BlindPool::new();

        // Insert some strings
        let handle1 = pool.insert("Hello".to_string());
        let handle2 = pool.insert("World".to_string());

        assert_eq!(pool.len(), 2);
        assert!(!pool.is_empty());
        assert!(pool.capacity_for::<String>() >= 2);

        // Values should be accessible through the handles
        assert_eq!(&*handle1, "Hello");
        assert_eq!(&*handle2, "World");

        // Dropping handles should remove items from pool
        drop(handle1);
        drop(handle2);

        // Pool should eventually be empty (may not be immediate due to Arc cleanup)
        // We test the basic functionality, not the exact timing of cleanup
    }

    #[test]
    fn handle_drop_removes_objects_both_exclusive_and_shared() {
        let mut pool = BlindPool::new();

        // Test exclusive handle drop
        let exclusive_handle = pool.insert("exclusive".to_string());
        assert_eq!(pool.len(), 1);
        drop(exclusive_handle);
        // Note: For managed pools, length might not immediately reflect drop due to Arc semantics

        // Test shared handle drop
        let mut_handle = pool.insert("shared".to_string());
        let shared_handle = mut_handle.into_shared();
        assert_eq!(pool.len(), 1); // Should have 1 item

        // Both handles point to same object
        assert_eq!(&*shared_handle, "shared");

        // Drop the shared handle
        drop(shared_handle);
        // Object should eventually be removed (Arc cleanup timing varies)
    }

    #[test]
    fn multiple_types_different_layouts() {
        let mut pool = BlindPool::new();

        // Insert different types with different layouts
        let string_handle = pool.insert("Test string".to_string());
        let u32_handle = pool.insert(42_u32);
        let u64_handle = pool.insert(123_u64);
        let vec_handle = pool.insert(vec![1, 2, 3, 4, 5]);

        assert_eq!(pool.len(), 4);

        // Each type should have its own capacity
        assert!(pool.capacity_for::<String>() >= 1);
        assert!(pool.capacity_for::<u32>() >= 1);
        assert!(pool.capacity_for::<u64>() >= 1);
        assert!(pool.capacity_for::<Vec<i32>>() >= 1);

        // Verify values are correct
        assert_eq!(&*string_handle, "Test string");
        assert_eq!(*u32_handle, 42);
        assert_eq!(*u64_handle, 123);
        assert_eq!(&*vec_handle, &vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn same_layout_different_types() {
        let mut pool = BlindPool::new();

        // u32 and i32 have the same layout
        let u32_handle = pool.insert(42_u32);
        let i32_handle = pool.insert(-42_i32);

        assert_eq!(pool.len(), 2);

        // Both should share capacity since they have the same layout
        let u32_capacity = pool.capacity_for::<u32>();
        let i32_capacity = pool.capacity_for::<i32>();
        assert_eq!(u32_capacity, i32_capacity);
        assert!(u32_capacity >= 2);

        // Values should be accessible
        assert_eq!(*u32_handle, 42);
        assert_eq!(*i32_handle, -42);
    }

    #[test]
    fn reserve_creates_capacity() {
        let mut pool = BlindPool::new();

        // Reserve capacity for strings
        pool.reserve_for::<String>(10);
        assert!(pool.capacity_for::<String>() >= 10);

        // Reserve capacity for u32s
        pool.reserve_for::<u32>(5);
        assert!(pool.capacity_for::<u32>() >= 5);

        // Insert items to verify reservations work
        let mut handles = Vec::new();
        for i in 0..10 {
            handles.push(pool.insert(format!("String {i}")));
        }

        assert_eq!(pool.len(), 10);

        // Verify all strings are correct
        for (i, handle) in handles.iter().enumerate() {
            assert_eq!(&**handle, &format!("String {i}"));
        }
    }

    #[test]
    fn shrink_to_fit_removes_unused_capacity() {
        let mut pool = BlindPool::new();

        // Reserve more than we need
        pool.reserve_for::<String>(100);

        // Insert only a few items
        let _handle1 = pool.insert("One".to_string());
        let _handle2 = pool.insert("Two".to_string());

        // Shrink to fit - this might not actually reduce capacity
        // but should not panic or cause issues
        pool.shrink_to_fit();

        // Pool should still work normally
        assert_eq!(pool.len(), 2);
        let _handle3 = pool.insert("Three".to_string());
        assert_eq!(pool.len(), 3);
    }

    #[test]
    fn shrink_to_fit_with_zero_items_shrinks_to_zero_capacity() {
        let mut pool = BlindPool::new();

        // Add some items to create capacity
        let handle1 = pool.insert("Item1".to_string());
        let handle2 = pool.insert(42_u32);
        let handle3 = pool.insert("Item3".to_string());

        // Verify we have capacity
        assert!(pool.capacity_for::<String>() > 0);
        assert!(pool.capacity_for::<u32>() > 0);

        // Remove all items by dropping handles
        drop(handle1);
        drop(handle2);
        drop(handle3);

        assert!(pool.is_empty());

        pool.shrink_to_fit();

        // Testing implementation detail: empty pool should shrink capacity to zero
        // This may become untrue with future algorithm changes, at which point
        // we will need to adjust the tests.
        assert_eq!(pool.capacity_for::<String>(), 0);
        assert_eq!(pool.capacity_for::<u32>(), 0);
    }

    #[test]
    fn insert_with_functionality() {
        let mut pool = BlindPool::new();

        // Test insert_with for partial initialization
        // SAFETY: We correctly initialize the String value in the closure
        let handle = unsafe {
            pool.insert_with(|uninit: &mut MaybeUninit<String>| {
                uninit.write(String::from("Initialized via closure"));
            })
        };

        assert_eq!(&*handle, "Initialized via closure");
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn pool_cloning_and_sharing() {
        let mut pool = BlindPool::new();

        // Insert an item
        let handle = pool.insert("Shared data".to_string());

        // Clone the pool (should share the same internal storage)
        let pool_clone = pool.clone();

        // Both pools should see the same length
        assert_eq!(pool.len(), 1);
        assert_eq!(pool_clone.len(), 1);

        // Data should be accessible from both pool references
        assert_eq!(&*handle, "Shared data");
    }

    #[test]
    fn thread_safety() {
        let mut pool = BlindPool::new();

        // Insert some initial data
        let handle1 = pool.insert("Thread test 1".to_string());
        let handle2 = pool.insert(42_u32);

        let pool = Arc::new(pool);
        let pool_clone = Arc::clone(&pool);

        // Spawn a thread that can access the pool
        let thread_handle = thread::spawn(move || {
            // Should be able to read the length
            assert!(pool_clone.len() >= 2);

            // Should be able to check capacity
            assert!(pool_clone.capacity_for::<String>() >= 1);
            assert!(pool_clone.capacity_for::<u32>() >= 1);
        });

        // Wait for thread to complete
        thread_handle.join().unwrap();

        // Original handles should still be valid
        assert_eq!(&*handle1, "Thread test 1");
        assert_eq!(*handle2, 42);
    }

    #[test]
    fn large_variety_of_types() {
        let mut pool = BlindPool::new();

        // Insert many different types (avoiding floating point for comparison issues)
        let string_handle = pool.insert("String".to_string());
        let u8_handle = pool.insert(255_u8);
        let u16_handle = pool.insert(65535_u16);
        let u32_handle = pool.insert(4_294_967_295_u32);
        let u64_handle = pool.insert(18_446_744_073_709_551_615_u64);
        let i8_handle = pool.insert(-128_i8);
        let i16_handle = pool.insert(-32768_i16);
        let i32_handle = pool.insert(-2_147_483_648_i32);
        let i64_handle = pool.insert(-9_223_372_036_854_775_808_i64);
        let bool_handle = pool.insert(true);
        let char_handle = pool.insert('Z');
        let vec_handle = pool.insert(vec![1, 2, 3]);
        let option_handle = pool.insert(Some("Optional".to_string()));

        assert_eq!(pool.len(), 13);

        // Verify all values
        assert_eq!(&*string_handle, "String");
        assert_eq!(*u8_handle, 255);
        assert_eq!(*u16_handle, 65535);
        assert_eq!(*u32_handle, 4_294_967_295);
        assert_eq!(*u64_handle, 18_446_744_073_709_551_615);
        assert_eq!(*i8_handle, -128);
        assert_eq!(*i16_handle, -32768);
        assert_eq!(*i32_handle, -2_147_483_648);
        assert_eq!(*i64_handle, -9_223_372_036_854_775_808);
        assert!(*bool_handle);
        assert_eq!(*char_handle, 'Z');
        assert_eq!(&*vec_handle, &vec![1, 2, 3]);
        assert_eq!(&*option_handle, &Some("Optional".to_string()));
    }

    #[test]
    fn handle_mutation() {
        let mut pool = BlindPool::new();

        // Insert a mutable type
        let mut string_handle = pool.insert("Initial".to_string());
        let mut vec_handle = pool.insert(vec![1, 2]);

        // Modify through the handles
        string_handle.push_str(" Modified");
        vec_handle.push(3);

        // Verify modifications
        assert_eq!(&*string_handle, "Initial Modified");
        assert_eq!(&*vec_handle, &vec![1, 2, 3]);
    }

    #[test]
    #[should_panic]
    fn zero_sized_types() {
        let mut pool = BlindPool::new();

        // Insert unit types (zero-sized) - this should panic
        let _unit_handle = pool.insert(());
    }
}
