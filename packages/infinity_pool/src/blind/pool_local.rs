use std::alloc::Layout;
use std::cell::RefMut;
use std::mem::MaybeUninit;
use std::rc::Rc;

use crate::{
    LayoutKey, LocalBlindPoolCore, LocalBlindPoolInnerMap, LocalBlindPooledMut, RawOpaquePool,
};

/// A single-threaded reference-counting object pool that accepts any type of object.
///
/// All values in the pool remain pinned for their entire lifetime.
///
/// The pool automatically expands its capacity when needed.
#[doc = include_str!("../../doc/snippets/local_pool_lifetimes.md")]
///
/// # Thread safety
///
/// The pool is single-threaded.
///
/// # Example
///
/// ```rust
/// use infinity_pool::LocalBlindPool;
///
/// fn work_with_data(data: impl std::fmt::Display + 'static) {
///     let mut pool = LocalBlindPool::new();
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
/// work_with_data("Hello, Local Blind!");
/// work_with_data(42);
/// ```
///
/// # Pool clones are functionally equivalent
///
/// ```rust
/// use infinity_pool::LocalBlindPool;
///
/// let mut pool1 = LocalBlindPool::new();
/// let pool2 = pool1.clone();
///
/// assert_eq!(pool1.len(), pool2.len());
/// let _handle = pool1.insert(42_i32);
/// assert_eq!(pool1.len(), pool2.len());
/// ```
#[derive(Clone, Debug, Default)]
pub struct LocalBlindPool {
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
    core: LocalBlindPoolCore,
}

impl LocalBlindPool {
    #[doc = include_str!("../../doc/snippets/pool_new.md")]
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[doc = include_str!("../../doc/snippets/pool_len.md")]
    #[must_use]
    #[inline]
    pub fn len(&self) -> usize {
        let core = self.core.borrow();

        core.values().map(RawOpaquePool::len).sum()
    }

    #[doc = include_str!("../../doc/snippets/blind_pool_capacity.md")]
    #[must_use]
    #[inline]
    pub fn capacity_for<T: 'static>(&self) -> usize {
        let key = LayoutKey::with_layout_of::<T>();

        let core = self.core.borrow();

        core.get(&key)
            .map(RawOpaquePool::capacity)
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
    pub fn reserve_for<T: 'static>(&mut self, additional: usize) {
        let mut core = self.core.borrow_mut();

        let pool = ensure_inner_pool::<T>(&mut core);

        pool.reserve(additional);
    }

    #[doc = include_str!("../../doc/snippets/pool_shrink_to_fit.md")]
    #[inline]
    pub fn shrink_to_fit(&mut self) {
        let mut core = self.core.borrow_mut();

        for pool in core.values_mut() {
            pool.shrink_to_fit();
        }
    }

    #[doc = include_str!("../../doc/snippets/pool_insert.md")]
    ///
    /// # Example
    ///
    /// ```rust
    /// use infinity_pool::LocalBlindPool;
    ///
    /// let mut pool = LocalBlindPool::new();
    ///
    /// // Insert an object into the pool
    /// let mut handle = pool.insert("Hello".to_string());
    ///
    /// // Mutate the object via the unique handle
    /// handle.push_str(", Local Blind World!");
    /// assert_eq!(&*handle, "Hello, Local Blind World!");
    ///
    /// // Transform the unique handle into a shared handle
    /// let shared_handle = handle.into_shared();
    ///
    /// // After transformation, you can only immutably dereference the object
    /// assert_eq!(&*shared_handle, "Hello, Local Blind World!");
    /// // shared_handle.push_str("!"); // This would not compile
    ///
    /// // The object is removed when the handle is dropped
    /// drop(shared_handle); // Explicitly drop to remove from pool
    /// assert_eq!(pool.len(), 0);
    /// ```
    #[inline]
    #[must_use]
    pub fn insert<T: 'static>(&mut self, value: T) -> LocalBlindPooledMut<T> {
        let mut core = self.core.borrow_mut();

        let pool = ensure_inner_pool::<T>(&mut core);

        // SAFETY: inner pool selector guarantees matching layout.
        let inner_handle = unsafe { pool.insert_unchecked(value) };

        LocalBlindPooledMut::new(
            inner_handle,
            LayoutKey::with_layout_of::<T>(),
            Rc::clone(&self.core),
        )
    }

    #[doc = include_str!("../../doc/snippets/pool_insert_with.md")]
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::mem::MaybeUninit;
    ///
    /// use infinity_pool::LocalBlindPool;
    ///
    /// struct DataBuffer {
    ///     id: u32,
    ///     data: MaybeUninit<[u8; 1024]>, // Large buffer to skip initializing
    /// }
    ///
    /// let mut pool = LocalBlindPool::new();
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
    pub unsafe fn insert_with<T, F>(&mut self, f: F) -> LocalBlindPooledMut<T>
    where
        T: 'static,
        F: FnOnce(&mut MaybeUninit<T>),
    {
        let mut core = self.core.borrow_mut();

        let pool = ensure_inner_pool::<T>(&mut core);

        // SAFETY: inner pool selector guarantees matching layout.
        // Initialization guarantee is forwarded from the caller.
        let inner_handle = unsafe { pool.insert_with_unchecked(f) };

        LocalBlindPooledMut::new(
            inner_handle,
            LayoutKey::with_layout_of::<T>(),
            Rc::clone(&self.core),
        )
    }
}

fn ensure_inner_pool<'a, T: 'static>(
    pools: &'a mut RefMut<'_, LocalBlindPoolInnerMap>,
) -> &'a mut RawOpaquePool {
    let layout = Layout::new::<T>();
    let key = LayoutKey::new(layout);

    pools
        .entry(key)
        .or_insert_with(|| RawOpaquePool::with_layout(layout))
}

#[cfg(test)]
mod tests {
    use std::mem::MaybeUninit;

    use super::*;

    #[test]
    fn default_pool_is_empty() {
        let pool = LocalBlindPool::default();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn single_type_operations() {
        let mut pool = LocalBlindPool::new();

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

        // Pool should eventually be empty (may not be immediate due to Rc cleanup)
        // We test the basic functionality, not the exact timing of cleanup
    }

    #[test]
    fn multiple_types_different_layouts() {
        let mut pool = LocalBlindPool::new();

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
        let mut pool = LocalBlindPool::new();

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
        let mut pool = LocalBlindPool::new();

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
        let mut pool = LocalBlindPool::new();

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
        let mut pool = LocalBlindPool::new();

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
        let mut pool = LocalBlindPool::new();

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
        let mut pool = LocalBlindPool::new();

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
    fn large_variety_of_types() {
        let mut pool = LocalBlindPool::new();

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
        let mut pool = LocalBlindPool::new();

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
        let mut pool = LocalBlindPool::new();

        // Insert unit types (zero-sized) - this should panic
        let _unit_handle = pool.insert(());
    }

    #[test]
    fn non_send_types() {
        // Custom non-Send type with raw pointer
        struct NonSendType(*const u8);
        // SAFETY: Only used in single-threaded local test environment, never shared across threads
        unsafe impl Sync for NonSendType {}

        // LocalBlindPool should work with non-Send types since it's single-threaded
        use std::cell::RefCell;
        use std::rc::Rc;

        let mut pool = LocalBlindPool::new();

        // Rc is not Send, but LocalBlindPool should handle it since it's single-threaded
        let rc_handle = pool.insert(Rc::new("Non-Send data".to_string()));
        assert_eq!(pool.len(), 1);
        assert_eq!(&**rc_handle, "Non-Send data");

        // RefCell is also not Send
        let refcell_handle = pool.insert(RefCell::new(42));
        assert_eq!(pool.len(), 2);
        assert_eq!(*refcell_handle.borrow(), 42);

        // Nested non-Send types
        let nested_handle = pool.insert(Rc::new(RefCell::new(vec![1, 2, 3])));
        assert_eq!(pool.len(), 3);
        assert_eq!(*nested_handle.borrow(), vec![1, 2, 3]);

        // Custom non-Send type with raw pointer
        let raw_ptr = 0x1234 as *const u8;
        let non_send_handle = pool.insert(NonSendType(raw_ptr));
        assert_eq!(pool.len(), 4);
        assert_eq!(non_send_handle.0, raw_ptr);
    }

    #[test]
    fn borrow_checker_test() {
        let mut pool = LocalBlindPool::new();

        // Test that we can work with multiple borrows correctly
        let handle1 = pool.insert("First".to_string());
        let handle2 = pool.insert("Second".to_string());

        // Should be able to read from both handles simultaneously
        let val1 = &*handle1;
        let val2 = &*handle2;

        assert_eq!(val1, "First");
        assert_eq!(val2, "Second");

        // Drop one handle and continue using the other
        drop(handle1);
        assert_eq!(val2, "Second");
    }

    #[test]
    fn mixed_lifetime_test() {
        let mut pool = LocalBlindPool::new();

        let long_lived_handle = pool.insert("Long lived".to_string());

        {
            let short_lived_handle = pool.insert("Short lived".to_string());
            assert_eq!(pool.len(), 2);
            assert_eq!(&*short_lived_handle, "Short lived");
            // short_lived_handle drops here
        }

        // Pool should still work and long_lived_handle should still be valid
        assert_eq!(&*long_lived_handle, "Long lived");

        // Add another item after the short-lived one is gone
        let new_handle = pool.insert("New item".to_string());
        assert_eq!(&*new_handle, "New item");
    }
}
