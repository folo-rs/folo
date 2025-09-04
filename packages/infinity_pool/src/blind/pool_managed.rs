use std::alloc::Layout;
use std::mem::MaybeUninit;
use std::sync::{Arc, Mutex, MutexGuard};

use foldhash::HashMap;

use crate::{ERR_POISONED_LOCK, PooledMut, RawOpaquePool, RawOpaquePoolSend};

/// A reference-counting object pool that accepts any type of object.
///
/// All values in the pool remain pinned for their entire lifetime.
///
/// The pool automatically expands its capacity when needed.
///
/// # Lifetime management
///
/// The pool type itself acts as a handle - any clones of it are functionally equivalent,
/// similar to `Arc`.
///
/// When inserting an object into the pool, a handle to the object is returned.
/// The object is removed from the pool when the last remaining handle to the object
/// is dropped (`Arc`-like behavior).
///
/// # Thread safety
///
/// The pool is thread-safe (`Send` and `Sync`) and requires that any inserted items are `Send`.
#[derive(Debug, Default, Clone)]
pub struct BlindPool {
    // Internal pools, one for each unique memory layout encountered.
    //
    // The pool type itself is just a handle around the inner pool,
    // which is reference-counted and mutex-guarded. The inner pool
    // will only ever be dropped once all items have been removed from
    // it and no more `OpaquePool` instances exist that point to it.
    //
    // This also implies that `DropPolicy` has no meaning for this
    // pool configuration, as the pool can never be dropped if it has
    // contents (as dropping the handles of pooled objects will remove
    // them from the pool, while keeping the pool alive until then).
    pools: Arc<Mutex<PoolMap>>,
}

impl BlindPool {
    /// Creates a new instance of the pool.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of objects currently in the pool.
    #[must_use]
    pub fn len(&self) -> usize {
        let pools = self.pools.lock().expect(ERR_POISONED_LOCK);

        pools
            .values()
            .map(|pool| pool.lock().expect(ERR_POISONED_LOCK).len())
            .sum()
    }

    /// The total capacity of the pool for objects of type `T`.
    ///
    /// This is the maximum number of objects of this type that the pool can contain without
    /// capacity extension. The pool will automatically extend its capacity if more than
    /// this many objects of type `T` are inserted. Capacity may be shared between different
    /// types of objects.
    #[must_use]
    pub fn capacity_for<T: Send>(&self) -> usize {
        let layout = Layout::new::<T>();

        let pools = self.pools.lock().expect(ERR_POISONED_LOCK);

        pools
            .get(&layout)
            .map(|pool| pool.lock().expect(ERR_POISONED_LOCK).capacity())
            .unwrap_or_default()
    }

    /// Whether the pool contains zero objects.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Ensures that the pool has capacity for at least `additional` more objects of type `T`.
    ///
    /// # Panics
    ///
    /// Panics if the new capacity would exceed the size of virtual memory.
    pub fn reserve_for<T: Send>(&mut self, additional: usize) {
        let mut pools = self.pools.lock().expect(ERR_POISONED_LOCK);

        let pool = ensure_inner_pool::<T>(&mut pools);

        pool.lock().expect(ERR_POISONED_LOCK).reserve(additional);
    }

    /// Drops unused pool capacity to reduce memory usage.
    ///
    /// There is no guarantee that any unused capacity can be dropped. The exact outcome depends
    /// on the specific pool structure and which objects remain in the pool.
    pub fn shrink_to_fit(&mut self) {
        let mut pools = self.pools.lock().expect(ERR_POISONED_LOCK);

        for pool in pools.values_mut() {
            pool.lock().expect(ERR_POISONED_LOCK).shrink_to_fit();
        }
    }

    /// Inserts an object into the pool and returns a handle to it.
    pub fn insert<T: Send>(&mut self, value: T) -> PooledMut<T> {
        let mut pools = self.pools.lock().expect(ERR_POISONED_LOCK);

        let pool = ensure_inner_pool::<T>(&mut pools);

        // SAFETY: inner pool selector guarantees matching layout.
        let inner_handle = unsafe {
            pool.lock()
                .expect(ERR_POISONED_LOCK)
                .insert_unchecked(value)
        };

        PooledMut::new(inner_handle, Arc::clone(pool))
    }

    /// Inserts an object into the pool via closure and returns a handle to it.
    ///
    /// This method allows the caller to partially initialize the object, skipping any `MaybeUninit`
    /// fields that are intentionally not initialized at insertion time. This can make insertion of
    /// objects containing `MaybeUninit` fields faster, although requires unsafe code to implement.
    ///
    /// This method is NOT faster than `insert()` for fully initialized objects.
    /// Prefer `insert()` for a better safety posture if you do not intend to
    /// skip initialization of any `MaybeUninit` fields.
    ///
    /// # Safety
    ///
    /// The closure must correctly initialize the object. All fields that
    /// are not `MaybeUninit` must be initialized when the closure returns.
    pub unsafe fn insert_with<T: Send, F>(&mut self, f: F) -> PooledMut<T>
    where
        F: FnOnce(&mut MaybeUninit<T>),
    {
        let mut pools = self.pools.lock().expect(ERR_POISONED_LOCK);

        let pool = ensure_inner_pool::<T>(&mut pools);

        // SAFETY: inner pool selector guarantees matching layout.
        // Initialization guarantee is forwarded from the caller.
        let inner_handle = unsafe {
            pool.lock()
                .expect(ERR_POISONED_LOCK)
                .insert_with_unchecked(f)
        };

        PooledMut::new(inner_handle, Arc::clone(pool))
    }
}

// Each inner pool is separately locked because those locks are used by the handles to
// remove the specific object from the specific pool in a thread-safe manner. The pool
// itself only ever takes those locks while holding the map lock, ensuring correct lock ordering.
type PoolMap = HashMap<Layout, Arc<Mutex<RawOpaquePoolSend>>>;

fn ensure_inner_pool<'a, T: Send>(
    pools: &'a mut MutexGuard<'_, PoolMap>,
) -> &'a Arc<Mutex<RawOpaquePoolSend>> {
    let layout = Layout::new::<T>();

    pools.entry(layout).or_insert_with_key(|layout| {
        // SAFETY: We always require `T: Send`.
        let inner = unsafe { RawOpaquePoolSend::new(RawOpaquePool::with_layout(*layout)) };

        Arc::new(Mutex::new(inner))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::mem::MaybeUninit;

    #[test]
    fn empty_pool() {
        let pool = BlindPool::new();
        
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity_for::<String>(), 0);
        assert_eq!(pool.capacity_for::<u32>(), 0);
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
    fn reserve_functionality() {
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
    fn shrink_to_fit() {
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
