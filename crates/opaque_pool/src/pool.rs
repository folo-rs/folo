use std::alloc::Layout;
use std::mem::ManuallyDrop;
use std::num::NonZero;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{DropPolicy, OpaqueSlab};

/// Global counter for generating unique pool IDs.
static POOL_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generates a unique pool ID.
fn generate_pool_id() -> u64 {
    POOL_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// A memory pool of unbounded size that inserts typed values into pinned memory.
///
/// The pool returns a [`Pooled<T>`] for each inserted value, which acts as both
/// the key and provides direct access to the memory pointer.
///
/// # Out of band access
///
/// The collection does not create or keep references to the memory blocks, so it is valid to access
/// memory via pointers and to create custom references to memory from unsafe code even when not
/// holding an exclusive reference to the collection.
///
/// # Resource usage
///
/// As of today, the collection never shrinks, though future versions may offer facilities to do so.
///
/// # Example
///
/// ```rust
/// use std::alloc::Layout;
///
/// use opaque_pool::OpaquePool;
///
/// let layout = Layout::new::<u32>();
/// let mut pool = OpaquePool::builder().layout(layout).build();
///
/// // Insert a value and get a handle.
/// // SAFETY: u32 matches the layout used to create the pool.
/// let pooled = unsafe { pool.insert(42u32) };
///
/// // Read from the memory.
/// // SAFETY: The pointer is valid and the memory contains the value we just inserted.
/// let value = unsafe { pooled.ptr().read() };
/// assert_eq!(value, 42);
///
/// // Remove the value from the pool.
/// pool.remove(pooled);
/// ```
#[derive(Debug)]
pub struct OpaquePool {
    /// We need to uniquely identify each pool to ensure that memory is not returned to the
    /// wrong pool. If the pool ID does not match when returning memory, we panic.
    pool_id: u64,

    /// The layout of memory blocks managed by this pool.
    item_layout: Layout,

    slab_capacity: NonZero<usize>,

    /// We use a Vec here to allow for dynamic capacity growth.
    ///
    /// For now, we only grow this Vec but in theory, one could implement shrinking as well
    /// by removing empty slabs.
    slabs: Vec<OpaqueSlab>,

    /// Lowest index of any slab that has a vacant slot, if known. We use this to avoid scanning
    /// the entire collection for vacant slots when reserving memory. This being `None` does not
    /// imply that there are no vacant slots, it just means we do not know what slab they are in.
    /// In other words, this is a cache, not the ground truth - we set it to `None` when we lose
    /// confidence that the data is still valid but when we have no need to look up the new value.
    slab_with_vacant_slot_index: Option<usize>,

    /// Drop policy that determines how the pool handles remaining items when dropped.
    drop_policy: DropPolicy,
}

/// Today, we assemble the pool from memory slabs, each containing a fixed number of memory blocks.
///
/// In the future, we may choose to be smarter about this, e.g. choosing the slab size dynamically
/// based on the size of the memory layout in order to match a memory page size, or another similar criterion.
/// This is why the parameter is also not exposed in the public API - we may want to change how we
/// perform the memory layout in a future version.
#[cfg(not(miri))]
pub(crate) const DEFAULT_SLAB_CAPACITY: usize = 128;

// Under Miri, we use a smaller slab capacity because Miri test runtime scales by memory usage.
#[cfg(miri)]
pub(crate) const DEFAULT_SLAB_CAPACITY: usize = 16;

impl OpaquePool {
    /// Creates a builder for configuring and constructing an [`OpaquePool`].
    ///
    /// This is the preferred way to create an [`OpaquePool`] as it allows configuring
    /// the drop policy and other options. You must specify a layout using either 
    /// `.layout()` or `.layout_of::<T>()` before calling `.build()`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use opaque_pool::OpaquePool;
    ///
    /// // Create a pool for storing u64 values using explicit layout.
    /// let layout = Layout::new::<u64>();
    /// let pool = OpaquePool::builder()
    ///     .layout(layout)
    ///     .build();
    ///
    /// assert_eq!(pool.len(), 0);
    /// assert!(pool.is_empty());
    /// assert_eq!(pool.item_layout(), layout);
    ///
    /// // Create a pool for storing u32 values using type-based layout.
    /// let pool = OpaquePool::builder()
    ///     .layout_of::<u32>()
    ///     .build();
    /// ```
    pub fn builder() -> crate::OpaquePoolBuilder {
        crate::OpaquePoolBuilder::new()
    }

    /// Creates a new [`OpaquePool`] with the specified configuration.
    ///
    /// This method is used internally by the builder to construct the actual pool.
    ///
    /// # Panics
    ///
    /// Panics if the layout has zero size.
    #[must_use]
    pub(crate) fn new_inner(
        item_layout: Layout,
        drop_policy: DropPolicy,
        slab_capacity: NonZero<usize>,
    ) -> Self {
        assert!(
            item_layout.size() > 0,
            "OpaquePool must have non-zero memory block size"
        );

        Self {
            pool_id: generate_pool_id(),
            item_layout,
            slab_capacity,
            slabs: Vec::new(),
            slab_with_vacant_slot_index: None,
            drop_policy,
        }
    }

    /// Returns the memory layout used by items in this pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use opaque_pool::OpaquePool;
    ///
    /// let layout = Layout::new::<u128>();
    /// let pool = OpaquePool::builder().layout(layout).build();
    ///
    /// assert_eq!(pool.item_layout(), layout);
    /// assert_eq!(pool.item_layout().size(), std::mem::size_of::<u128>());
    /// ```
    #[must_use]
    pub fn item_layout(&self) -> Layout {
        self.item_layout
    }

    /// The number of values in the pool that have been inserted.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use opaque_pool::OpaquePool;
    ///
    /// let layout = Layout::new::<i32>();
    /// let mut pool = OpaquePool::builder().layout(layout).build();
    ///
    /// assert_eq!(pool.len(), 0);
    ///
    /// // SAFETY: i32 matches the layout used to create the pool.
    /// let pooled1 = unsafe { pool.insert(1i32) };
    /// assert_eq!(pool.len(), 1);
    ///
    /// // SAFETY: i32 matches the layout used to create the pool.
    /// let pooled2 = unsafe { pool.insert(2i32) };
    /// assert_eq!(pool.len(), 2);
    ///
    /// pool.remove(pooled1);
    /// assert_eq!(pool.len(), 1);
    /// # pool.remove(pooled2);
    /// ```
    #[must_use]
    pub fn len(&self) -> usize {
        self.slabs.iter().map(OpaqueSlab::len).sum()
    }

    /// The number of values the pool can accommodate without additional resource allocation.
    ///
    /// This is the total capacity, including any existing insertions. The capacity may grow
    /// automatically when [`insert()`] is called and no space is available.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use opaque_pool::OpaquePool;
    ///
    /// let layout = Layout::new::<u8>();
    /// let mut pool = OpaquePool::builder().layout(layout).build();
    ///
    /// // New pool starts with zero capacity.
    /// assert_eq!(pool.capacity(), 0);
    ///
    /// // Inserting values may increase capacity.
    /// // SAFETY: u8 matches the layout used to create the pool.
    /// let pooled = unsafe { pool.insert(42u8) };
    /// assert!(pool.capacity() > 0);
    /// assert!(pool.capacity() >= pool.len());
    /// # pool.remove(pooled);
    /// ```
    ///
    /// [`insert()`]: Self::insert
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.slabs
            .len()
            .checked_mul(self.slab_capacity.get())
            .expect("capacity calculation cannot overflow for reasonable slab counts")
    }

    /// Whether the pool has no inserted values.
    ///
    /// An empty pool may still be holding unused memory capacity.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use opaque_pool::OpaquePool;
    ///
    /// let layout = Layout::new::<u16>();
    /// let mut pool = OpaquePool::builder().layout(layout).build();
    ///
    /// assert!(pool.is_empty());
    ///
    /// // SAFETY: u16 matches the layout used to create the pool.
    /// let pooled = unsafe { pool.insert(42u16) };
    /// assert!(!pool.is_empty());
    ///
    /// pool.remove(pooled);
    /// assert!(pool.is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slabs.iter().all(OpaqueSlab::is_empty)
    }

    /// Inserts a value into the pool and returns a handle that acts as both the key and pointer.
    ///
    /// The returned [`Pooled<T>`] provides direct access to the memory via [`Pooled::ptr()`]
    /// and must be returned to the pool via [`remove()`] to free the memory and properly drop the value.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use opaque_pool::OpaquePool;
    ///
    /// let layout = Layout::new::<u64>();
    /// let mut pool = OpaquePool::builder().layout(layout).build();
    ///
    /// // Insert a value.
    /// // SAFETY: u64 matches the layout used to create the pool.
    /// let pooled = unsafe { pool.insert(0xDEADBEEF_CAFEBABEu64) };
    ///
    /// // Read data back.
    /// let value = unsafe { pooled.ptr().read() };
    /// assert_eq!(value, 0xDEADBEEF_CAFEBABE);
    ///
    /// // Must remove the value to free the memory and drop it properly.
    /// pool.remove(pooled);
    /// ```
    ///
    /// # Panics
    ///
    /// In debug builds, panics if the layout of `T` does not match the pool's item layout.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the layout of `T` matches the pool's item layout.
    /// In debug builds, this is checked with an assertion.
    ///
    /// [`remove()`]: Self::remove
    #[must_use]
    pub unsafe fn insert<T>(&mut self, value: T) -> Pooled<T> {
        let slab_index = self.index_of_slab_with_vacant_slot();
        let slab = self
            .slabs
            .get_mut(slab_index)
            .expect("we just verified that there is a slab with a vacant slot at this index");

        // We invalidate the "slab with vacant slot" cache here if this is the last vacant slot.
        let predicted_slab_filled_slots = slab
            .len()
            .checked_add(1)
            .expect("we cannot overflow because there is at least one free slot, so it means there must be room to increment");

        if predicted_slab_filled_slots == self.slab_capacity.get() {
            self.slab_with_vacant_slot_index = None;
        }

        // SAFETY: The caller ensures T's layout matches the pool's layout.
        let pooled = unsafe { slab.insert(value) };
        let coordinates = MemoryBlockCoordinates::from_parts(
            slab_index,
            pooled.index(),
            self.slab_capacity.get(),
        );

        Pooled {
            pool_id: self.pool_id,
            coordinates,
            ptr: pooled.ptr().cast::<T>(),
        }
    }

    /// Removes a value previously inserted into the pool.
    ///
    /// The [`Pooled<T>`] is consumed by this operation and cannot be used afterward.
    /// The value is properly dropped and the memory becomes available for future insertions.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use opaque_pool::OpaquePool;
    ///
    /// let layout = Layout::new::<i32>();
    /// let mut pool = OpaquePool::builder().layout(layout).build();
    ///
    /// // SAFETY: i32 matches the layout used to create the pool.
    /// let pooled = unsafe { pool.insert(42i32) };
    /// assert_eq!(pool.len(), 1);
    ///
    /// // Remove the value.
    /// pool.remove(pooled);
    /// assert_eq!(pool.len(), 0);
    /// assert!(pool.is_empty());
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the handle is not associated with a value in this pool.
    pub fn remove<T>(&mut self, pooled: Pooled<T>) {
        // Pooled has a no-execute `Drop` impl, so we drop it manually here.
        let pooled = ManuallyDrop::new(pooled);

        assert!(
            pooled.pool_id == self.pool_id,
            "attempted to remove a handle from a different pool (handle pool ID: {}, current pool ID: {})",
            pooled.pool_id,
            self.pool_id
        );

        let coordinates = pooled.coordinates;

        let Some(slab) = self.slabs.get_mut(coordinates.slab_index) else {
            panic!("handle was not associated with a value in the pool")
        };

        slab.remove(coordinates.index_in_slab);

        // There is now a vacant slot in this slab! We may want to remember this for fast insertions.
        // We try to remember the lowest index of a slab with a vacant slot, so we
        // fill the collection from the start (to enable easier shrinking later).
        if self
            .slab_with_vacant_slot_index
            .is_none_or(|current| current > coordinates.slab_index)
        {
            self.slab_with_vacant_slot_index = Some(coordinates.slab_index);
        }
    }

    #[must_use]
    fn index_of_slab_with_vacant_slot(&mut self) -> usize {
        if let Some(index) = self.slab_with_vacant_slot_index {
            // If we have this cached, we return it immediately.
            // This is a performance optimization to avoid scanning the entire collection.
            return index;
        }

        // We lookup the first slab with some free space, filling the collection from the start.
        let index = if let Some((index, _)) = self
            .slabs
            .iter()
            .enumerate()
            .find(|(_, slab)| !slab.is_full())
        {
            index
        } else {
            // All slabs are full, so we need to expand capacity.
            self.slabs
                .push(OpaqueSlab::new(self.item_layout, self.slab_capacity, self.drop_policy));

            self.slabs
                .len()
                .checked_sub(1)
                .expect("we just pushed a slab, so this cannot overflow because len >= 1")
        };

        // We update the cache. The caller is responsible for invalidating this when needed.
        self.slab_with_vacant_slot_index = Some(index);
        index
    }

    #[cfg_attr(test, mutants::skip)] // This is essentially test logic, mutation is meaningless.
    #[cfg(debug_assertions)]
    #[expect(dead_code, reason = "we will probably use it later")]
    pub(crate) fn integrity_check(&self) {
        for slab in &self.slabs {
            slab.integrity_check();
        }
    }
}

/// The result of inserting a value of type `T` into a [`OpaquePool`].
///
/// Acts as both the handle and the key - the user must return this to the pool to remove
/// the value and properly drop it. The pool will panic on drop if some active handles remain.
///
/// The generic parameter `T` provides type-safe access to the stored value through [`ptr()`](Pooled::ptr).
/// If you need to erase the type information, use [`erase()`](Pooled::erase) to convert to `Pooled<()>`.
///
/// # Example
///
/// ```rust
/// use std::alloc::Layout;
///
/// use opaque_pool::OpaquePool;
///
/// let layout = Layout::new::<i64>();
/// let mut pool = OpaquePool::builder().layout(layout).build();
///
/// // SAFETY: i64 matches the layout used to create the pool.
/// let pooled = unsafe { pool.insert(-123i64) };
///
/// // Read from the memory pointer.
/// let value = unsafe { pooled.ptr().read() };
/// assert_eq!(value, -123);
///
/// // The handle must be returned to remove the value and drop it properly.
/// pool.remove(pooled);
/// ```
#[derive(Debug)]
pub struct Pooled<T> {
    /// Ensures this handle can only be returned to the pool it came from.
    pool_id: u64,

    coordinates: MemoryBlockCoordinates,

    ptr: NonNull<T>,
}

impl<T> Pooled<T> {
    /// Returns a pointer to the inserted value.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use opaque_pool::OpaquePool;
    ///
    /// let layout = Layout::new::<f64>();
    /// let mut pool = OpaquePool::builder().layout(layout).build();
    ///
    /// // SAFETY: f64 matches the layout used to create the pool.
    /// let pooled = unsafe { pool.insert(3.14159f64) };
    ///
    /// // Read data back from the memory.
    /// let value = unsafe { pooled.ptr().read() };
    /// assert_eq!(value, 3.14159);
    /// # pool.remove(pooled);
    /// ```
    #[must_use]
    pub fn ptr(&self) -> NonNull<T> {
        self.ptr
    }

    /// Erases the type information from this [`Pooled<T>`] handle, returning a [`Pooled<()>`].
    ///
    /// This is useful when you want to store handles of different types in the same collection
    /// or pass them to code that doesn't need to know the specific type.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use opaque_pool::OpaquePool;
    ///
    /// let layout = Layout::new::<u64>();
    /// let mut pool = OpaquePool::builder().layout(layout).build();
    ///
    /// // SAFETY: u64 matches the layout used to create the pool.
    /// let pooled = unsafe { pool.insert(42u64) };
    ///
    /// // Erase type information.
    /// let erased = pooled.erase();
    ///
    /// // Can still access the raw pointer.
    /// // SAFETY: We know this contains a u64.
    /// let value = unsafe { erased.ptr().cast::<u64>().read() };
    /// assert_eq!(value, 42);
    /// # pool.remove(erased);
    /// ```
    #[must_use]
    pub fn erase(self) -> Pooled<()> {
        Pooled {
            pool_id: self.pool_id,
            coordinates: self.coordinates,
            ptr: self.ptr.cast::<()>(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct MemoryBlockCoordinates {
    slab_index: usize,
    index_in_slab: usize,
}

impl MemoryBlockCoordinates {
    #[must_use]
    fn from_parts(slab: usize, index_in_slab: usize, _slab_capacity: usize) -> Self {
        Self {
            slab_index: slab,
            index_in_slab,
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block,
    clippy::items_after_statements,
    clippy::indexing_slicing,
    clippy::needless_range_loop,
    reason = "test code doesn't need the same safety rigor as production code"
)]
mod tests {
    use std::alloc::Layout;

    use super::*;

    #[test]
    fn smoke_test() {
        let layout = Layout::new::<u32>();
        let mut pool = OpaquePool::builder().layout(layout).build();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());

        // SAFETY: The layout of u32 matches the pool's layout.
        let pooled_a = unsafe { pool.insert(42_u32) };
        // SAFETY: The layout of u32 matches the pool's layout.
        let pooled_b = unsafe { pool.insert(43_u32) };
        // SAFETY: The layout of u32 matches the pool's layout.
        let pooled_c = unsafe { pool.insert(44_u32) };

        assert_eq!(pool.len(), 3);
        assert!(!pool.is_empty());
        assert!(pool.capacity() >= 3);

        // Read them back via pooled pointer.
        unsafe {
            assert_eq!(pooled_a.ptr().read(), 42);
            assert_eq!(pooled_b.ptr().read(), 43);
            assert_eq!(pooled_c.ptr().read(), 44);
        }

        pool.remove(pooled_b);

        // SAFETY: The layout of u32 matches the pool's layout.
        let pooled_d = unsafe { pool.insert(45_u32) };

        unsafe {
            assert_eq!(pooled_a.ptr().read(), 42);
            assert_eq!(pooled_c.ptr().read(), 44);
            assert_eq!(pooled_d.ptr().read(), 45);
        }

        // Clean up remaining pooled items.
        pool.remove(pooled_a);
        pool.remove(pooled_c);
        pool.remove(pooled_d);
    }

    #[test]
    #[should_panic]
    fn remove_nonexistent_panics() {
        let layout = Layout::new::<u32>();
        let mut pool = OpaquePool::builder().layout(layout).build();

        // Create a fake pooled with invalid coordinates.
        let fake_pooled: Pooled<u32> = Pooled {
            pool_id: pool.pool_id, // Use correct pool ID but invalid coordinates
            coordinates: MemoryBlockCoordinates {
                slab_index: 0,
                index_in_slab: 0,
            },
            ptr: NonNull::dangling(),
        };

        pool.remove(fake_pooled);
    }

    #[test]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "test uses small values that fit in u32"
    )]
    fn multi_slab_growth() {
        let layout = Layout::new::<u32>();
        let mut pool = OpaquePool::builder().layout(layout).build();

        // Reserve more items than a single slab can hold to test growth.
        // We use 2 * DEFAULT_SLAB_CAPACITY + 1 to guarantee we need at least 3 slabs.
        let items_to_reserve = 2 * DEFAULT_SLAB_CAPACITY + 1;
        let mut pooled_items = Vec::new();
        for i in 0..items_to_reserve {
            // SAFETY: The layout of u32 matches the pool's layout.
            let pooled = unsafe { pool.insert(i as u32) };
            pooled_items.push(pooled);
        }

        assert_eq!(pool.len(), items_to_reserve);
        assert!(pool.capacity() >= items_to_reserve);

        // Verify all values are still accessible.
        for (i, pooled) in pooled_items.iter().enumerate() {
            unsafe {
                assert_eq!(pooled.ptr().as_ptr().read(), i as u32);
            }
        }

        // Clean up all pooled items.
        for pooled in pooled_items {
            pool.remove(pooled);
        }
    }

    #[test]
    fn different_layouts() {
        // Test with different sized types.
        let layout_u64 = Layout::new::<u64>();
        let mut pool_u64 = OpaquePool::builder().layout(layout_u64).build();
        // SAFETY: The layout of u64 matches the pool's layout.
        let pooled = unsafe { pool_u64.insert(0x1234567890ABCDEF_u64) };
        unsafe {
            assert_eq!(pooled.ptr().as_ptr().read(), 0x1234567890ABCDEF);
        }
        pool_u64.remove(pooled);

        // Test with larger struct.
        #[repr(C)]
        struct LargeStruct {
            a: u64,
            b: u64,
            c: u64,
            d: u64,
        }

        let layout_large = Layout::new::<LargeStruct>();
        let mut pool_large = OpaquePool::builder().layout(layout_large).build();

        let test_struct = LargeStruct {
            a: 1,
            b: 2,
            c: 3,
            d: 4,
        };
        // SAFETY: The layout of LargeStruct matches the pool's layout.
        let pooled = unsafe { pool_large.insert(test_struct) };
        unsafe {
            let value = pooled.ptr().as_ptr().read();
            assert_eq!(value.a, 1);
            assert_eq!(value.b, 2);
            assert_eq!(value.c, 3);
            assert_eq!(value.d, 4);
        }
        pool_large.remove(pooled);
    }

    #[test]
    #[should_panic]
    fn zero_size_layout_is_panic() {
        let layout = Layout::from_size_align(0, 1).unwrap();
        drop(OpaquePool::builder().layout(layout).build());
    }

    #[test]
    fn stress_test_repeated_insert_remove() {
        let layout = Layout::new::<usize>();
        let mut pool = OpaquePool::builder().layout(layout).build();

        // Insert and remove many items to test slab management.
        for iteration in 0..10 {
            let mut pooled_items = Vec::new();
            for i in 0..50 {
                let value = iteration * 100 + i;
                // SAFETY: The layout of usize matches the pool's layout.
                let pooled = unsafe { pool.insert(value) };
                pooled_items.push(pooled);
            }

            // Remove every other item.
            let mut remaining_pooled = Vec::new();
            for (index, pooled) in pooled_items.into_iter().enumerate() {
                if index % 2 == 0 {
                    pool.remove(pooled);
                } else {
                    remaining_pooled.push(pooled);
                }
            }

            // Verify remaining items.
            for (index, pooled) in remaining_pooled.iter().enumerate() {
                let expected_value = iteration * 100 + (index * 2 + 1); // Odd indices
                unsafe {
                    assert_eq!(pooled.ptr().as_ptr().read(), expected_value);
                }
            }

            // Remove remaining items.
            for pooled in remaining_pooled {
                pool.remove(pooled);
            }
        }

        assert!(pool.is_empty());
    }

    #[test]
    fn drop_with_no_active_pooled_does_not_panic() {
        let layout = Layout::new::<u64>();
        let mut pool = OpaquePool::builder().layout(layout).build();

        // Insert and then remove immediately.
        // SAFETY: The layout of u64 matches the pool's layout.
        let pooled = unsafe { pool.insert(42_u64) };
        pool.remove(pooled);

        assert!(pool.is_empty());

        // Pool should drop without panic.
        drop(pool);
    }

    #[test]
    #[should_panic]
    fn drop_with_active_pooled_panics() {
        let layout = Layout::new::<u64>();
        let mut pool = OpaquePool::builder()
            .layout(layout)
            .drop_policy(DropPolicy::MustNotDropItems)
            .build();

        // Pooled items are undroppable, so we just let this pooled item leak to trigger the panic.
        // SAFETY: The layout of u64 matches the pool's layout.
        let _pooled = unsafe { pool.insert(42_u64) };

        // Pool should panic on drop since we still have an active pooled item.
        drop(pool);
    }

    #[test]
    #[should_panic]
    fn remove_pooled_from_different_pool_panics() {
        let layout = Layout::new::<u32>();
        let mut pool1 = OpaquePool::builder().layout(layout).build();
        let mut pool2 = OpaquePool::builder().layout(layout).build();

        // Insert into pool1 but try to remove from pool2.
        // SAFETY: The layout of u32 matches the pool's layout.
        let pooled = unsafe { pool1.insert(42_u32) };
        pool2.remove(pooled); // Should panic
    }

    #[test]
    fn pool_ids_are_unique() {
        let layout = Layout::new::<u32>();
        let pool1 = OpaquePool::builder().layout(layout).build();
        let pool2 = OpaquePool::builder().layout(layout).build();
        let pool3 = OpaquePool::builder().layout(layout).build();

        // Pool IDs should be different for each pool instance.
        assert_ne!(pool1.pool_id, pool2.pool_id);
        assert_ne!(pool2.pool_id, pool3.pool_id);
        assert_ne!(pool1.pool_id, pool3.pool_id);
    }

    #[test]
    fn pooled_belongs_to_correct_pool() {
        let layout = Layout::new::<u64>();
        let mut pool = OpaquePool::builder().layout(layout).build();

        // SAFETY: The layout of u64 matches the pool's layout.
        let pooled = unsafe { pool.insert(42_u64) };

        // The pooled item should have the same pool ID as the pool it came from.
        assert_eq!(pooled.pool_id, pool.pool_id);

        // Removing from the same pool should work fine.
        pool.remove(pooled);
    }

    #[test]
    fn pooled_erase_functionality() {
        let layout = Layout::new::<u32>();
        let mut pool = OpaquePool::builder().layout(layout).build();

        // SAFETY: The layout of u32 matches the pool's layout.
        let pooled = unsafe { pool.insert(42_u32) };

        // Test that the typed pointer works.
        unsafe {
            assert_eq!(pooled.ptr().read(), 42);
        }

        // Erase the type information.
        let erased = pooled.erase();

        // Should still be able to access the value through the erased pointer.
        unsafe {
            assert_eq!(erased.ptr().cast::<u32>().read(), 42);
        }

        // Should be able to remove the erased handle.
        pool.remove(erased);
    }

    #[test]
    fn drop_policy_may_drop_items_works() {
        let layout = Layout::new::<u32>();
        let mut pool = OpaquePool::builder()
            .layout(layout)
            .drop_policy(DropPolicy::MayDropItems)
            .build();

        // SAFETY: The layout of u32 matches the pool's layout.
        let _pooled = unsafe { pool.insert(42_u32) };

        // Pool should drop without panic even with active items when using MayDropItems
        drop(pool);
    }

    #[test]
    #[should_panic]
    fn drop_policy_must_not_drop_items_panics() {
        let layout = Layout::new::<u32>();
        let mut pool = OpaquePool::builder()
            .layout(layout)
            .drop_policy(DropPolicy::MustNotDropItems)
            .build();

        // SAFETY: The layout of u32 matches the pool's layout.
        let _pooled = unsafe { pool.insert(42_u32) };

        // Pool should panic on drop when using MustNotDropItems with active items
        drop(pool);
    }

    #[test]
    fn drop_policy_must_not_drop_items_ok_when_empty() {
        let layout = Layout::new::<u32>();
        let mut pool = OpaquePool::builder()
            .layout(layout)
            .drop_policy(DropPolicy::MustNotDropItems)
            .build();

        // SAFETY: The layout of u32 matches the pool's layout.
        let pooled = unsafe { pool.insert(42_u32) };
        
        // Remove the item before dropping
        pool.remove(pooled);

        // Pool should drop without panic when empty, even with MustNotDropItems
        drop(pool);
    }
}
