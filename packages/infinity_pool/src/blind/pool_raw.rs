use std::alloc::Layout;
use std::mem::MaybeUninit;

use foldhash::{HashMap, HashMapExt};

use crate::{DropPolicy, RawBlindPoolBuilder, RawBlindPooled, RawBlindPooledMut, RawOpaquePool};

/// An object pool that accepts any type of object.
///
/// All values in the pool remain pinned for their entire lifetime.
///
/// The pool automatically expands its capacity when needed.
///
/// # Thread safety
///
/// The pool is single-threaded, though if all the objects inserted are `Send` then the owner of
/// the pool is allowed to treat the pool itself as `Send` (but must do so via a wrapper type that
/// implements `Send` using unsafe code).
#[derive(Debug)]
pub struct RawBlindPool {
    /// Internal pools, one for each unique memory layout encountered.
    pools: HashMap<Layout, RawOpaquePool>,

    drop_policy: DropPolicy,
}

impl RawBlindPool {
    /// Starts configuring and creating a new instance of the pool.
    pub fn builder() -> RawBlindPoolBuilder {
        RawBlindPoolBuilder::new()
    }

    /// Creates a new instance of the pool with the default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::builder().build()
    }

    #[must_use]
    pub(crate) fn new_inner(drop_policy: DropPolicy) -> Self {
        Self {
            pools: HashMap::new(),
            drop_policy,
        }
    }

    /// The number of objects currently in the pool.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pools.values().map(RawOpaquePool::len).sum()
    }

    /// The total capacity of the pool for objects of type `T`.
    ///
    /// This is the maximum number of objects of this type that the pool can contain without
    /// capacity extension. The pool will automatically extend its capacity if more than
    /// this many objects of type `T` are inserted. Capacity may be shared between different
    /// types of objects.
    #[must_use]
    pub fn capacity_for<T>(&self) -> usize {
        self.inner_pool_of::<T>()
            .map(RawOpaquePool::capacity)
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
    pub fn reserve_for<T>(&mut self, additional: usize) {
        self.inner_pool_of_mut::<T>().reserve(additional);
    }

    /// Drops unused pool capacity to reduce memory usage.
    ///
    /// There is no guarantee that any unused capacity can be dropped. The exact outcome depends
    /// on the specific pool structure and which objects remain in the pool.
    pub fn shrink_to_fit(&mut self) {
        for pool in self.pools.values_mut() {
            pool.shrink_to_fit();
        }
    }

    /// Inserts an object into the pool and returns a handle to it.
    pub fn insert<T>(&mut self, value: T) -> RawBlindPooledMut<T> {
        let layout = Layout::new::<T>();
        let pool = self.inner_pool_mut(layout);

        // SAFETY: inner pool selector guarantees matching layout.
        let inner_handle = unsafe { pool.insert_unchecked(value) };

        RawBlindPooledMut::new(layout, inner_handle)
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
    pub unsafe fn insert_with<T, F>(&mut self, f: F) -> RawBlindPooledMut<T>
    where
        F: FnOnce(&mut MaybeUninit<T>),
    {
        let layout = Layout::new::<T>();
        let pool = self.inner_pool_mut(layout);

        // SAFETY: inner pool selector guarantees matching layout.
        // Initialization guarantee is forwarded from the caller.
        let inner_handle = unsafe { pool.insert_with_unchecked(f) };

        RawBlindPooledMut::new(layout, inner_handle)
    }

    /// Removes an object from the pool, dropping it.
    ///
    /// # Panics
    ///
    /// Panics if the handle does not reference an object in this pool.
    pub fn remove_mut<T: ?Sized>(&mut self, handle: RawBlindPooledMut<T>) {
        let pool = self.inner_pool_mut(handle.layout());

        pool.remove_mut(handle.into_inner());
    }

    /// Removes an object from the pool, dropping it.
    ///
    /// # Panics
    ///
    /// Panics if the handle does not reference an object in this pool.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the handle belongs to this pool and that the object it
    /// references has not already been removed from the pool.
    pub unsafe fn remove<T: ?Sized>(&mut self, handle: RawBlindPooled<T>) {
        let pool = self.inner_pool_mut(handle.layout());

        // SAFETY: Forwarding safety guarantees from the caller.
        unsafe {
            pool.remove(handle.into_inner());
        }
    }

    /// Removes an object from the pool and returns it.
    ///
    /// # Panics
    ///
    /// Panics if the handle does not reference an object in this pool.
    ///
    /// Panics if the object handle has been type-erased (`RawBlindPooledMut<()>`).
    #[must_use]
    pub fn remove_mut_unpin<T: Unpin>(&mut self, handle: RawBlindPooledMut<T>) -> T {
        // We would rather prefer to check for `RawBlindPooledMut<()>` specifically but
        // that would imply specialization or `T: 'static` or TypeId shenanigans.
        // This is good enough because type-erasing a handle is the only way to get a
        // handle to a ZST anyway because the slab does not even support ZSTs.
        assert_ne!(
            size_of::<T>(),
            0,
            "cannot remove_mut_unpin() from a blind pool through a type-erased handle"
        );

        let pool = self.inner_pool_of_mut::<T>();

        pool.remove_mut_unpin(handle.into_inner())
    }

    /// Removes an object from the pool and returns it.
    ///
    /// # Panics
    ///
    /// Panics if the handle does not reference an existing object in this pool.
    ///
    /// Panics if the object handle has been type-erased (`RawPooled<()>`).
    ///
    /// # Safety
    ///
    /// The caller must ensure that the handle belongs to this pool and that the object it
    /// references has not already been removed from the pool.
    #[must_use]
    pub unsafe fn remove_unpin<T: Unpin>(&mut self, handle: RawBlindPooled<T>) -> T {
        // We would rather prefer to check for `RawPooled<()>` specifically but
        // that would imply specialization or `T: 'static` or TypeId shenanigans.
        // This is good enough because type-erasing a handle is the only way to get a
        // handle to a ZST anyway because the slab does not even support ZSTs.
        assert_ne!(
            size_of::<T>(),
            0,
            "cannot remove_unpin() from a blind pool through a type-erased handle"
        );

        let pool = self.inner_pool_of_mut::<T>();

        // SAFETY: Forwarding safety guarantees from the caller.
        unsafe { pool.remove_unpin(handle.into_inner()) }
    }

    fn inner_pool_of<T>(&self) -> Option<&RawOpaquePool> {
        let layout = Layout::new::<T>();

        self.pools.get(&layout)
    }

    fn inner_pool_of_mut<T>(&mut self) -> &mut RawOpaquePool {
        let layout = Layout::new::<T>();

        self.inner_pool_mut(layout)
    }

    fn inner_pool_mut(&mut self, layout: Layout) -> &mut RawOpaquePool {
        self.pools.entry(layout).or_insert_with_key(|layout| {
            RawOpaquePool::builder()
                .drop_policy(self.drop_policy)
                .layout(*layout)
                .build()
        })
    }
}

impl Default for RawBlindPool {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::mem::MaybeUninit;

    use super::*;

    #[test]
    fn new_pool_is_empty() {
        let pool = RawBlindPool::new();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity_for::<String>(), 0);
        assert_eq!(pool.capacity_for::<u32>(), 0);
    }

    #[test]
    fn default_pool_is_empty() {
        let pool = RawBlindPool::default();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn instance_creation_through_builder_succeeds() {
        let pool = RawBlindPool::builder().build();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn insert_and_length() {
        let mut pool = RawBlindPool::new();

        let handle = pool.insert(42_u32);

        assert_eq!(pool.len(), 1);
        assert!(!pool.is_empty());
        assert_eq!(*handle, 42);
    }

    #[test]
    fn capacity_grows_when_needed() {
        let mut pool = RawBlindPool::new();

        assert_eq!(pool.capacity_for::<String>(), 0);

        let _handle1 = pool.insert("Hello".to_string());
        let capacity_after_first = pool.capacity_for::<String>();
        assert!(capacity_after_first >= 1);

        let _handle2 = pool.insert("World".to_string());
        let capacity_after_second = pool.capacity_for::<String>();
        assert!(capacity_after_second >= 2);
    }

    #[test]
    fn reserve_creates_capacity() {
        let mut pool = RawBlindPool::new();

        pool.reserve_for::<String>(10);
        assert!(pool.capacity_for::<String>() >= 10);

        pool.reserve_for::<u32>(5);
        assert!(pool.capacity_for::<u32>() >= 5);
    }

    #[test]
    fn insert_with_closure() {
        let mut pool = RawBlindPool::new();

        // SAFETY: We correctly initialize the String in the closure
        let handle = unsafe {
            pool.insert_with(|uninit: &mut MaybeUninit<String>| {
                uninit.write("initialized".to_string());
            })
        };

        assert_eq!(*handle, "initialized");
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn remove_decreases_length() {
        let mut pool = RawBlindPool::new();

        let handle = pool.insert(42_u32);
        assert_eq!(pool.len(), 1);

        pool.remove_mut(handle);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn remove_with_shared_handle() {
        let mut pool = RawBlindPool::new();

        let handle_mut = pool.insert(42_u32);
        let handle_shared = handle_mut.into_shared();

        // SAFETY: Handle belongs to this pool and hasn't been removed
        unsafe {
            pool.remove(handle_shared);
        }

        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn shrink_to_fit_reduces_unused_capacity() {
        let mut pool = RawBlindPool::new();

        // Reserve more than we need
        pool.reserve_for::<String>(100);

        // Insert only a few items
        let _handle1 = pool.insert("One".to_string());
        let _handle2 = pool.insert("Two".to_string());

        // Shrink should not panic
        pool.shrink_to_fit();

        // Pool should still work normally
        assert_eq!(pool.len(), 2);
        let _handle3 = pool.insert("Three".to_string());
        assert_eq!(pool.len(), 3);
    }

    #[test]
    fn shrink_to_fit_with_zero_items_shrinks_to_zero_capacity() {
        let mut pool = RawBlindPool::new();

        // Add some items to create capacity
        let handle1 = pool.insert("Item1".to_string());
        let handle2 = pool.insert(42_u32);
        let handle3 = pool.insert("Item3".to_string());

        // Verify we have capacity
        assert!(pool.capacity_for::<String>() > 0);
        assert!(pool.capacity_for::<u32>() > 0);

        // Remove all items
        pool.remove_mut(handle1);
        pool.remove_mut(handle2);
        pool.remove_mut(handle3);

        assert!(pool.is_empty());

        pool.shrink_to_fit();

        // Testing implementation detail: empty pool should shrink capacity to zero
        // This may become untrue with future algorithm changes, at which point
        // we will need to adjust the tests.
        assert_eq!(pool.capacity_for::<String>(), 0);
        assert_eq!(pool.capacity_for::<u32>(), 0);
    }

    #[test]
    fn handle_provides_access_to_object() {
        let mut pool = RawBlindPool::new();

        let string_handle = pool.insert("test".to_string());
        let u32_handle = pool.insert(42_u32);

        assert_eq!(*string_handle, "test");
        assert_eq!(*u32_handle, 42);
    }

    #[test]
    fn shared_handles_are_copyable() {
        let mut pool = RawBlindPool::new();

        let handle_mut = pool.insert(42_u32);
        let handle1 = handle_mut.into_shared();
        let handle2 = handle1;

        assert_eq!(*handle1, 42);
        assert_eq!(*handle2, 42);
    }

    #[test]
    fn multiple_removals_and_insertions() {
        let mut pool = RawBlindPool::new();

        // Insert multiple items
        let handle1 = pool.insert("one".to_string());
        let handle2 = pool.insert(2_u32);
        let handle3 = pool.insert("three".to_string());

        assert_eq!(pool.len(), 3);

        // Remove them
        pool.remove_mut(handle2);
        assert_eq!(pool.len(), 2);

        pool.remove_mut(handle1);
        assert_eq!(pool.len(), 1);

        pool.remove_mut(handle3);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn remove_unpin_returns_value() {
        let mut pool = RawBlindPool::new();

        let handle = pool.insert(42_u32);
        let value = pool.remove_mut_unpin(handle);

        assert_eq!(value, 42);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn remove_unpin_with_shared_handle() {
        let mut pool = RawBlindPool::new();

        let handle_mut = pool.insert(42_u32);
        let handle_shared = handle_mut.into_shared();

        // SAFETY: Handle belongs to this pool and hasn't been removed
        let value = unsafe { pool.remove_unpin(handle_shared) };

        assert_eq!(value, 42);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn multiple_types_different_layouts() {
        let mut pool = RawBlindPool::new();

        // Insert different types with different layouts
        let string_handle = pool.insert("test".to_string());
        let u32_handle = pool.insert(42_u32);
        let u64_handle = pool.insert(123_u64);
        let vec_handle = pool.insert(vec![1, 2, 3]);

        assert_eq!(pool.len(), 4);

        // Each type should have its own capacity
        assert!(pool.capacity_for::<String>() >= 1);
        assert!(pool.capacity_for::<u32>() >= 1);
        assert!(pool.capacity_for::<u64>() >= 1);
        assert!(pool.capacity_for::<Vec<i32>>() >= 1);

        // Verify values are correct
        assert_eq!(*string_handle, "test");
        assert_eq!(*u32_handle, 42);
        assert_eq!(*u64_handle, 123);
        assert_eq!(*vec_handle, vec![1, 2, 3]);
    }

    #[test]
    fn same_layout_different_types() {
        let mut pool = RawBlindPool::new();

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
    #[should_panic]
    fn zero_sized_types() {
        let mut pool = RawBlindPool::new();

        // Insert unit types (zero-sized) - this should panic
        let _unit_handle = pool.insert(());
    }
}
