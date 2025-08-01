use std::alloc::Layout;
use std::num::NonZero;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};

use new_zealand::nz;

use crate::{DropPolicy, OpaquePoolBuilder, OpaqueSlab};

/// Global counter for generating unique pool IDs.
static POOL_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generates a unique pool ID.
fn generate_pool_id() -> u64 {
    POOL_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// A pinned object pool of unbounded size that accepts objects of different types as long
/// as they match a specific memory layout.
///
/// The pool returns a [`Pooled<T>`] for each inserted value, which acts as a super-powered
/// pointer that can be copied and cloned freely. Each handle provides direct access to the
/// inserted item via a pointer.
///
/// # Out of band access
///
/// The collection does not create or keep references to the memory blocks. The only way to access
/// the contents of the collection is via unsafe code by using the pointer from a [`Pooled<T>`].
///
/// The collection does not create or maintain any `&` shared or `&mut` exclusive references to
/// the items it contains, except when explicitly called to operate on an item (e.g. `remove()`
/// implies exclusive access).
///
/// # Resource usage
///
/// The collection automatically grows as items are added. To reduce memory usage after items have
/// been removed, use the [`shrink_to_fit()`][1] method to release unused capacity.
///
/// [1]: Self::shrink_to_fit
///
/// # Example
///
/// ```rust
/// use std::alloc::Layout;
///
/// use opaque_pool::OpaquePool;
///
/// let mut pool = OpaquePool::builder().layout_of::<u32>().build();
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
/// // Remove the value from the pool. This invalidates the pointer and drops the value.
/// pool.remove(pooled);
/// ```
#[derive(Debug)]
pub struct OpaquePool {
    /// We need to uniquely identify each pool to ensure that handles are not returned to the
    /// wrong pool. If the pool ID does not match when a handle is returned, we panic.
    pool_id: u64,

    /// The memory layout of items in this pool. We accept items of any type as long as they
    /// match this layout.
    item_layout: Layout,

    /// We use a Vec here to allow for dynamic capacity growth.
    ///
    /// The Vec can grow as items are added and can shrink when empty slabs are removed via
    /// `shrink_to_fit()`.
    slabs: Vec<OpaqueSlab>,

    /// Lowest index of any slab that has a vacant slot, if known. We use this to avoid scanning
    /// the entire collection for vacant slots when reserving memory. This being `None` does not
    /// imply that there are no vacant slots, it just means we do not know what slab they are in.
    /// In other words, this is a cache, not the ground truth - we set it to `None` when we lose
    /// confidence that the data is still valid but when we have no need to look up the new value.
    slab_with_vacant_slot_index: Option<usize>,

    /// Drop policy that determines how the pool handles remaining items when dropped.
    drop_policy: DropPolicy,

    /// Number of items currently in the pool. We track this explicitly to avoid repeatedly
    /// summing across slabs when calculating the length.
    length: usize,
}

/// Today, we assemble the pool from memory slabs, each containing a fixed number of memory blocks.
///
/// In the future, we may choose to be smarter about this, e.g. choosing the slab size dynamically
/// based on the size of the memory layout in order to match a memory page size, or another similar criterion.
/// This is why the parameter is also not exposed in the public API - we may want to change how we
/// perform the memory layout in a future version.
#[cfg(not(miri))]
pub(crate) const DEFAULT_SLAB_CAPACITY: NonZero<usize> = nz!(128);

// Under Miri, we use a smaller slab capacity because Miri test runtime scales by memory usage.
#[cfg(miri)]
pub(crate) const DEFAULT_SLAB_CAPACITY: NonZero<usize> = nz!(16);

impl OpaquePool {
    /// Creates a builder for configuring and constructing an [`OpaquePool`].
    ///
    /// This how you can create an [`OpaquePool`]. You must specify an item memory layout
    /// using either  `.layout()` or `.layout_of::<T>()` before calling `.build()`.
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
    /// let pool = OpaquePool::builder().layout(layout).build();
    ///
    /// assert_eq!(pool.len(), 0);
    /// assert!(pool.is_empty());
    /// assert_eq!(pool.item_layout(), layout);
    ///
    /// // Create a pool for storing u32 values using type-based layout.
    /// let pool = OpaquePool::builder().layout_of::<u32>().build();
    /// ```
    pub fn builder() -> OpaquePoolBuilder {
        OpaquePoolBuilder::new()
    }

    /// Creates a new [`OpaquePool`] with the specified configuration.
    ///
    /// This method is used internally by the builder to construct the actual pool.
    ///
    /// # Panics
    ///
    /// Panics if the layout has zero size.
    #[must_use]
    pub(crate) fn new_inner(item_layout: Layout, drop_policy: DropPolicy) -> Self {
        assert!(
            item_layout.size() > 0,
            "OpaquePool must have non-zero item size"
        );

        Self {
            pool_id: generate_pool_id(),
            item_layout,
            slabs: Vec::new(),
            slab_with_vacant_slot_index: None,
            drop_policy,
            length: 0,
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

    /// The number of values that have been inserted into the pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use opaque_pool::OpaquePool;
    ///
    /// let mut pool = OpaquePool::builder().layout_of::<i32>().build();
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
    /// ```
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Can be mutated to infinitely growing memory use and/or infinite loop.
    pub fn len(&self) -> usize {
        debug_assert_eq!(self.length, self.slabs.iter().map(OpaqueSlab::len).sum());

        self.length
    }

    /// The number of values the pool can accommodate without additional resource allocation.
    ///
    /// This is the total capacity, including any existing items. The capacity will grow
    /// automatically when [`insert()`] is called and insufficient capacity is available.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use opaque_pool::OpaquePool;
    ///
    /// let mut pool = OpaquePool::builder().layout_of::<u8>().build();
    ///
    /// // New pool starts with zero capacity.
    /// assert_eq!(pool.capacity(), 0);
    ///
    /// // Inserting values may increase capacity.
    /// // SAFETY: u8 matches the layout used to create the pool.
    /// let pooled = unsafe { pool.insert(42u8) };
    ///
    /// assert!(pool.capacity() > 0);
    /// assert!(pool.capacity() >= pool.len());
    /// ```
    ///
    /// [`insert()`]: Self::insert
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.slabs
            .len()
            .checked_mul(DEFAULT_SLAB_CAPACITY.get())
            .expect(
                "overflow here would imply capacity is greater than virtual memory - impossible",
            )
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
    /// let mut pool = OpaquePool::builder().layout_of::<u16>().build();
    ///
    /// assert!(pool.is_empty());
    ///
    /// // SAFETY: u16 matches the layout used to create the pool.
    /// let pooled = unsafe { pool.insert(42u16) };
    ///
    /// assert!(!pool.is_empty());
    ///
    /// pool.remove(pooled);
    /// assert!(pool.is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Reserves capacity for at least `additional` more items to be inserted in the pool.
    ///
    /// The pool may reserve more space to speculatively avoid frequent reallocations.
    /// After calling `reserve`, capacity will be greater than or equal to
    /// `self.len() + additional`. Does nothing if capacity is already sufficient.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use opaque_pool::OpaquePool;
    ///
    /// let mut pool = OpaquePool::builder().layout_of::<u32>().build();
    ///
    /// // Reserve space for 10 more items
    /// pool.reserve(10);
    /// assert!(pool.capacity() >= 10);
    ///
    /// // SAFETY: u32 matches the layout used to create the pool.
    /// let pooled = unsafe { pool.insert(42u32) };
    ///
    /// // Reserve additional space on top of existing items
    /// pool.reserve(5);
    /// assert!(pool.capacity() >= pool.len() + 5);
    /// ```
    #[cfg_attr(test, mutants::skip)] // Can be mutated to infinitely growing memory use and/or infinite loop.
    pub fn reserve(&mut self, additional: usize) {
        let required_capacity = self
            .len()
            .checked_add(additional)
            .expect("capacity overflow: requested capacity exceeds maximum possible value");

        if self.capacity() >= required_capacity {
            return;
        }

        // Calculate how many additional slabs we need
        let current_slabs = self.slabs.len();
        let required_slabs = required_capacity.div_ceil(DEFAULT_SLAB_CAPACITY.get());
        let additional_slabs = required_slabs.saturating_sub(current_slabs);

        for _ in 0..additional_slabs {
            self.slabs.push(OpaqueSlab::new(
                self.item_layout,
                DEFAULT_SLAB_CAPACITY,
                self.drop_policy,
            ));
        }
    }

    /// Shrinks the pool's memory usage by dropping unused capacity.
    ///
    /// This method reduces the pool's memory footprint by removing unused capacity
    /// where possible. Items currently in the pool are preserved.
    ///
    /// The pool's capacity may be reduced, but all existing handles remain valid.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use opaque_pool::OpaquePool;
    ///
    /// let mut pool = OpaquePool::builder().layout_of::<u32>().build();
    ///
    /// // Insert some items to create slabs
    /// // SAFETY: u32 matches the layout used to create the pool.
    /// let pooled1 = unsafe { pool.insert(1u32) };
    /// // SAFETY: u32 matches the layout used to create the pool.
    /// let pooled2 = unsafe { pool.insert(2u32) };
    /// let initial_capacity = pool.capacity();
    ///
    /// // Remove all items
    /// pool.remove(pooled1);
    /// pool.remove(pooled2);
    ///
    /// // Capacity remains the same until we shrink
    /// assert_eq!(pool.capacity(), initial_capacity);
    ///
    /// // Shrink to fit reduces capacity
    /// pool.shrink_to_fit();
    /// assert!(pool.capacity() <= initial_capacity);
    /// ```
    #[cfg_attr(test, mutants::skip)] // Too annoying to test the vacant index caching.
    pub fn shrink_to_fit(&mut self) {
        // Find the last non-empty slab by scanning from the end
        let new_len = self
            .slabs
            .iter()
            .enumerate()
            .rev()
            .find_map(|(idx, slab)| {
                if !slab.is_empty() {
                    Some(idx.checked_add(1).expect("slab index cannot overflow"))
                } else {
                    None
                }
            })
            .unwrap_or(0);

        // If we're about to remove slabs, we need to invalidate the vacant slot cache
        // since it might point to a slab that will no longer exist
        if new_len < self.slabs.len() {
            self.slab_with_vacant_slot_index = None;
        }

        // Truncate the slabs vector to remove empty slabs from the end
        self.slabs.truncate(new_len);
    }

    /// Inserts a value into the pool and returns a handle that acts as the key and supplies
    /// a pointer to the item.
    ///
    /// The returned [`Pooled<T>`] provides direct access to the memory via [`Pooled::ptr()`].
    /// Accessing this pointer from unsafe code is the only way to use the inserted value.
    ///
    /// The [`Pooled<T>`] may be returned to the pool via [`remove()`] to free the memory and
    /// drop the value. Behavior of the pool if dropped when non-empty is determined
    /// by the pool's [drop policy][DropPolicy].
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use opaque_pool::OpaquePool;
    ///
    /// let mut pool = OpaquePool::builder().layout_of::<u64>().build();
    ///
    /// // Insert a value.
    /// // SAFETY: u64 matches the layout used to create the pool.
    /// let pooled = unsafe { pool.insert(0xDEADBEEF_CAFEBABEu64) };
    ///
    /// // Read data back.
    /// // SAFETY: The pointer is valid for u64 reads/writes and we have exclusive access.
    /// let value = unsafe { pooled.ptr().read() };
    /// assert_eq!(value, 0xDEADBEEF_CAFEBABE);
    ///
    /// // Write a new value into the item.
    /// // SAFETY: The pointer is valid for u64 reads/writes and we have exclusive access.
    /// unsafe {
    ///     pooled.ptr().write(0xBEEFCAFE_DEADBEEFu64);
    /// }
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

        if predicted_slab_filled_slots == DEFAULT_SLAB_CAPACITY.get() {
            self.slab_with_vacant_slot_index = None;
        }

        // SAFETY: The caller ensures T's layout matches the pool's layout.
        let pooled = unsafe { slab.insert(value) };
        let coordinates = ItemCoordinates::from_parts(slab_index, pooled.index());

        // Update our tracked length since we just inserted an item.
        self.length = self
            .length
            .checked_add(1)
            .expect("length overflow: pool cannot contain more items than usize::MAX");

        // The pool itself does not care about the type T but for the convenience of the caller
        // we imbue the Pooled with the type information, to reduce required casting by caller.
        Pooled {
            pool_id: self.pool_id,
            coordinates,
            ptr: pooled.ptr().cast::<T>(),
        }
    }

    /// Removes a value previously inserted into the pool.
    ///
    /// The [`Pooled<T>`] is consumed by this operation and cannot be used afterward.
    /// The value is dropped and the memory becomes available for future insertions.
    ///
    /// There is no way to remove an item from the pool without dropping it.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use opaque_pool::OpaquePool;
    ///
    /// let mut pool = OpaquePool::builder().layout_of::<i32>().build();
    ///
    /// // SAFETY: i32 matches the layout used to create the pool.
    /// let pooled = unsafe { pool.insert(42i32) };
    /// assert_eq!(pool.len(), 1);
    ///
    /// // Remove the value.
    /// pool.remove(pooled);
    ///
    /// assert_eq!(pool.len(), 0);
    /// assert!(pool.is_empty());
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the handle is not associated with this pool.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "intentionally consuming the handle"
    )]
    pub fn remove<T>(&mut self, pooled: Pooled<T>) {
        assert!(
            pooled.pool_id == self.pool_id,
            "attempted to remove a handle from a different pool (handle pool ID: {}, current pool ID: {})",
            pooled.pool_id,
            self.pool_id
        );

        let coordinates = pooled.coordinates;

        let slab = self
            .slabs
            .get_mut(coordinates.slab_index)
            .expect("a slab cannot be removed without first removing all items in it");

        // In principle, we could return the value here if `T: Unpin` but there is no need
        // for this functionality at present, so we do not implement it to reduce complexity.
        slab.remove(coordinates.index_in_slab);

        // Update our tracked length since we just removed an item.
        self.length = self
            .length
            .checked_sub(1)
            .expect("length underflow: cannot remove more items than exist in pool");

        // There is now a vacant slot in this slab! We may want to remember this for fast insertions.
        // We try to remember the lowest index of a slab with a vacant slot, so we
        // fill the collection from the start (to enable easier shrinking later).
        self.update_vacant_slot_cache(coordinates.slab_index);
    }

    /// Adds a new slab to the pool and returns its index.
    #[must_use]
    fn add_new_slab(&mut self) -> usize {
        self.slabs.push(OpaqueSlab::new(
            self.item_layout,
            DEFAULT_SLAB_CAPACITY,
            self.drop_policy,
        ));

        self.slabs
            .len()
            .checked_sub(1)
            .expect("we just pushed a slab, so this cannot overflow because len >= 1")
    }

    #[must_use]
    fn index_of_slab_with_vacant_slot(&mut self) -> usize {
        if let Some(index) = self.slab_with_vacant_slot_index {
            // If we have this cached, we return it immediately.
            // This is a performance optimization to avoid scanning the entire collection.
            return index;
        }

        // If the pool is full, we know we need to add a new slab without checking.
        if self.len() == self.capacity() {
            let index = self.add_new_slab();
            self.set_vacant_slot_cache(index);
            return index;
        }

        // We lookup the first slab with some free space, filling the collection from the start.
        let index = self
            .slabs
            .iter()
            .enumerate()
            .find_map(|(index, slab)| if !slab.is_full() { Some(index) } else { None })
            .expect("since len() != capacity(), at least one slab must have vacant slots");

        // We update the cache. The caller is responsible for invalidating this when needed.
        self.set_vacant_slot_cache(index);
        index
    }

    /// Updates the vacant slot cache to point to the slab with the lowest index that has a vacant slot.
    ///
    /// This should be called when a slot becomes vacant in a slab. The cache will only be updated
    /// if the provided slab index is lower than the current cached index, ensuring we always
    /// point to the lowest-indexed slab with vacant slots for better memory locality.
    #[cfg_attr(test, mutants::skip)] // Some mutations are untestable - this is just a cache so even if this gets mutated away, we will still operate correctly, just with less performance.
    fn update_vacant_slot_cache(&mut self, slab_with_vacant_slot_index: usize) {
        if self
            .slab_with_vacant_slot_index
            .is_none_or(|current| current > slab_with_vacant_slot_index)
        {
            self.slab_with_vacant_slot_index = Some(slab_with_vacant_slot_index);
        }
    }

    /// Sets the vacant slot cache to the specified slab index.
    ///
    /// This unconditionally updates the cache and should be used when we have determined
    /// the exact slab index that should be cached.
    #[cfg_attr(test, mutants::skip)] // Some mutations are untestable - this is just a cache so even if this gets mutated away, we will still operate correctly, just with less performance.
    fn set_vacant_slot_cache(&mut self, slab_index: usize) {
        self.slab_with_vacant_slot_index = Some(slab_index);
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
/// Acts as a super-powered pointer that can be copied and cloned freely. The handle serves
/// both as the key and provides direct access to the stored value. You can return this to
/// the pool to remove the value from the pool and drop it. Depending on the pool's
/// [drop policy][DropPolicy], the pool may panic if it is dropped while still containing items.
///
/// Being `Copy` and `Clone`, this type behaves like a regular pointer - you can duplicate
/// handles freely without affecting the underlying stored value. Multiple copies of the same
/// handle all refer to the same stored value.
///
/// The generic parameter `T` provides type-safe access to the stored value through
/// [`ptr()`](Pooled::ptr). If you need to erase the type information, use
/// [`erase()`](Pooled::erase) to convert the instance to a `Pooled<()>`, which is
/// functionally equivalent.
///
/// # Example
///
/// ```rust
/// use std::alloc::Layout;
///
/// use opaque_pool::OpaquePool;
///
/// let mut pool = OpaquePool::builder().layout_of::<i64>().build();
///
/// // SAFETY: i64 matches the layout used to create the pool.
/// let pooled = unsafe { pool.insert(-123i64) };
///
/// // The handle acts like a super-powered pointer - it can be copied freely.
/// let pooled_copy = pooled;
/// let pooled_clone = pooled.clone();
///
/// // All copies refer to the same stored value.
/// // SAFETY: All pointers are valid and point to the same value.
/// let value1 = unsafe { pooled.ptr().read() };
/// let value2 = unsafe { pooled_copy.ptr().read() };
/// let value3 = unsafe { pooled_clone.ptr().read() };
/// assert_eq!(value1, -123);
/// assert_eq!(value2, -123);
/// assert_eq!(value3, -123);
///
/// // To remove and drop an item, any handle can be returned to the pool.
/// pool.remove(pooled);
/// ```
#[derive(Clone, Copy, Debug)]
pub struct Pooled<T> {
    /// Ensures this handle can only be returned to the pool it came from.
    pool_id: u64,

    coordinates: ItemCoordinates,

    ptr: NonNull<T>,
}

impl<T> Pooled<T> {
    /// Returns a pointer to the inserted value.
    ///
    /// This is the only way to access the value stored in the pool. The owner of the handle has
    /// exclusive access to the value and may both read and write and may create both `&` shared
    /// and `&mut` exclusive references to the item.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use opaque_pool::OpaquePool;
    ///
    /// let mut pool = OpaquePool::builder().layout_of::<f64>().build();
    ///
    /// // SAFETY: f64 matches the layout used to create the pool.
    /// let pooled = unsafe { pool.insert(3.14159f64) };
    ///
    /// // Read data back from the memory.
    /// let value = unsafe { pooled.ptr().read() };
    /// assert_eq!(value, 3.14159);
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
    /// The handle remains functionally equivalent and can still be used to remove the item
    /// from the pool and drop it. The only change is the removal of the type information.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use opaque_pool::OpaquePool;
    ///
    /// let mut pool = OpaquePool::builder().layout_of::<u64>().build();
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
    ///
    /// // Can still remove the item.
    /// pool.remove(erased);
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
struct ItemCoordinates {
    slab_index: usize,
    index_in_slab: usize,
}

impl ItemCoordinates {
    #[must_use]
    fn from_parts(slab: usize, index_in_slab: usize) -> Self {
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
    clippy::cast_possible_truncation,
    reason = "tests focus on succinct code and do not need to tick all the boxes"
)]
mod tests {
    use std::alloc::Layout;

    use super::*;

    #[test]
    fn smoke_test() {
        let mut pool = OpaquePool::builder().layout_of::<u32>().build();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());

        let pooled_a = unsafe { pool.insert(42_u32) };
        let pooled_b = unsafe { pool.insert(43_u32) };
        let pooled_c = unsafe { pool.insert(44_u32) };

        assert_eq!(pool.len(), 3);
        assert!(!pool.is_empty());
        assert!(pool.capacity() >= 3);

        unsafe {
            assert_eq!(pooled_a.ptr().read(), 42);
            assert_eq!(pooled_b.ptr().read(), 43);
            assert_eq!(pooled_c.ptr().read(), 44);
        }

        pool.remove(pooled_b);

        let pooled_d = unsafe { pool.insert(45_u32) };

        unsafe {
            assert_eq!(pooled_a.ptr().read(), 42);
            assert_eq!(pooled_c.ptr().read(), 44);
            assert_eq!(pooled_d.ptr().read(), 45);
        }

        pool.remove(pooled_a);
        pool.remove(pooled_d);
        // We do not remove pooled_c, leaving that up to the pool to clean up.
    }

    #[test]
    #[should_panic]
    fn remove_nonexistent_panics() {
        let layout = Layout::new::<u32>();
        let mut pool = OpaquePool::builder().layout(layout).build();

        // Create a fake pooled with invalid coordinates.
        let fake_pooled: Pooled<u32> = Pooled {
            pool_id: pool.pool_id, // Use correct pool ID but invalid coordinates
            coordinates: ItemCoordinates {
                slab_index: 0,
                index_in_slab: 0,
            },
            ptr: NonNull::dangling(),
        };

        pool.remove(fake_pooled);
    }

    #[test]
    fn multi_slab_growth() {
        let mut pool = OpaquePool::builder().layout_of::<u32>().build();

        // Reserve more items than a single slab can hold to test growth.
        // We use 2 * DEFAULT_SLAB_CAPACITY + 1 to guarantee we need at least 3 slabs.
        let items_to_reserve = 2 * DEFAULT_SLAB_CAPACITY.get() + 1;

        let mut pooled_items = Vec::with_capacity(items_to_reserve);

        for i in 0..items_to_reserve {
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
    }

    #[test]
    #[should_panic]
    fn zero_size_layout_is_panic() {
        let layout = Layout::from_size_align(0, 1).unwrap();
        drop(OpaquePool::builder().layout(layout).build());
    }

    #[test]
    fn drop_with_no_active_pooled_does_not_panic_if_policy_must_not_drop() {
        let mut pool = OpaquePool::builder()
            .layout_of::<u64>()
            .drop_policy(DropPolicy::MustNotDropItems)
            .build();

        // Insert and then remove immediately.
        let pooled = unsafe { pool.insert(42_u64) };
        pool.remove(pooled);

        assert!(pool.is_empty());

        // Pool should drop without panic.
        drop(pool);
    }

    #[test]
    #[should_panic]
    fn drop_with_active_pooled_panics_if_policy_must_not_drop() {
        let layout = Layout::new::<u64>();
        let mut pool = OpaquePool::builder()
            .layout(layout)
            .drop_policy(DropPolicy::MustNotDropItems)
            .build();

        _ = unsafe { pool.insert(42_u64) };

        // Based on policy, pool should panic on drop since we still have an item in the pool.
        drop(pool);
    }

    #[test]
    #[should_panic]
    fn remove_pooled_from_different_pool_panics() {
        let layout = Layout::new::<u32>();
        let mut pool1 = OpaquePool::builder().layout(layout).build();
        let mut pool2 = OpaquePool::builder().layout(layout).build();

        // Insert into pool1 but try to remove from pool2.
        let pooled1 = unsafe { pool1.insert(42_u32) };

        // We also insert to pool2 to ensure there is something to remove in there.
        // The removal should still fail - having an item there is not enough.
        _ = unsafe { pool2.insert(42_u32) };

        pool2.remove(pooled1); // Should panic.
    }

    #[test]
    fn pooled_erase_functionality() {
        let layout = Layout::new::<u32>();
        let mut pool = OpaquePool::builder().layout(layout).build();

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
    fn mixed_types_same_layout() {
        use std::f64::consts::PI;

        // Define a transparent wrapper around u64 to test struct types.
        #[repr(transparent)]
        struct WrappedU64(u64);

        // All these types have the same layout as u64.
        let layout = Layout::new::<u64>();
        let mut pool = OpaquePool::builder().layout(layout).build();

        // Insert different types with the same layout.
        let pooled_u64 = unsafe { pool.insert(0xDEAD_BEEF_CAFE_BABE_u64) };
        let pooled_i64 = unsafe { pool.insert(-1_234_567_890_123_456_789_i64) };
        let pooled_f64 = unsafe { pool.insert(PI) };
        let pooled_wrapped = unsafe { pool.insert(WrappedU64(0x1234_5678_90AB_CDEF_u64)) };

        assert_eq!(pool.len(), 4);

        // Verify all values are accessible and correct.
        unsafe {
            assert_eq!(pooled_u64.ptr().read(), 0xDEAD_BEEF_CAFE_BABE);
            assert_eq!(pooled_i64.ptr().read(), -1_234_567_890_123_456_789);
            assert!((pooled_f64.ptr().read() - PI).abs() < f64::EPSILON);
            assert_eq!(pooled_wrapped.ptr().read().0, 0x1234_5678_90AB_CDEF);
        }

        // Test cross-type access by casting pointers (demonstrating layout compatibility).
        unsafe {
            // Read u64 value as raw bytes and verify it matches when cast to other types.
            let u64_as_bytes = pooled_u64.ptr().cast::<[u8; 8]>().read();
            let expected_bytes = 0xDEAD_BEEF_CAFE_BABE_u64.to_ne_bytes();
            assert_eq!(u64_as_bytes, expected_bytes);

            // Read i64 value and verify it has the expected bit pattern.
            let i64_value = pooled_i64.ptr().read();
            let i64_as_u64 = pooled_i64.ptr().cast::<u64>().read();
            #[expect(
                clippy::cast_sign_loss,
                reason = "intentionally testing bit-level equivalence"
            )]
            let expected_u64 = i64_value as u64;
            assert_eq!(i64_as_u64, expected_u64);

            // Read f64 value and verify it can be accessed as u64 bits.
            let f64_value = pooled_f64.ptr().read();
            let f64_as_u64 = pooled_f64.ptr().cast::<u64>().read();
            assert_eq!(f64_as_u64, f64_value.to_bits());

            // Read wrapped struct and verify it can be accessed as plain u64.
            let wrapped_value = pooled_wrapped.ptr().read();
            let wrapped_as_u64 = pooled_wrapped.ptr().cast::<u64>().read();
            assert_eq!(wrapped_as_u64, wrapped_value.0);
        }

        // Remove items in different order to test that handles work correctly.
        pool.remove(pooled_f64);
        pool.remove(pooled_u64);
        assert_eq!(pool.len(), 2);

        // Verify remaining items are still accessible.
        unsafe {
            assert_eq!(pooled_i64.ptr().read(), -1_234_567_890_123_456_789);
            assert_eq!(pooled_wrapped.ptr().read().0, 0x1234_5678_90AB_CDEF);
        }

        pool.remove(pooled_wrapped);
        pool.remove(pooled_i64);
        assert!(pool.is_empty());
    }

    #[test]
    fn fill_first_slab_before_allocating_second() {
        let mut pool = OpaquePool::builder().layout_of::<u32>().build();

        for _ in 0..DEFAULT_SLAB_CAPACITY.get() {
            _ = unsafe { pool.insert(1234_u32) };
        }

        assert_eq!(pool.slabs.len(), 1);
        assert!(pool.slabs[0].is_full());

        // This will allocate a second slab.
        _ = unsafe { pool.insert(1234_u32) };

        assert_eq!(pool.slabs.len(), 2);
    }

    #[test]
    fn fill_hole_before_allocating_new_slab() {
        let mut pool = OpaquePool::builder().layout_of::<u32>().build();

        // Fill the first slab.
        let mut pooled_items = Vec::new();
        for _ in 0..DEFAULT_SLAB_CAPACITY.get() {
            pooled_items.push(unsafe { pool.insert(1234_u32) });
        }

        // Remove the first item to create a hole.
        let first_item = pooled_items.remove(0);
        pool.remove(first_item);

        // This will fill the hole instead of allocating a new slab.
        let pooled_filled = unsafe { pool.insert(5678_u32) };

        assert_eq!(pooled_filled.coordinates.slab_index, 0);
        assert_eq!(pooled_filled.coordinates.index_in_slab, 0);
        unsafe {
            assert_eq!(pooled_filled.ptr().read(), 5678);
        }

        // Clean up remaining items.
        for item in pooled_items {
            pool.remove(item);
        }
        pool.remove(pooled_filled);
    }

    #[test]
    fn fill_first_hole_ascending() {
        // If two slabs have a hole, we always fill a hole in the first (index-wise) slab.
        // We do not care which hole we fill (there may be multiple per slab), we just care
        // about which slab it is in.
        //
        // We create the holes in ascending order (first slab first, then second slab).

        let mut pool = OpaquePool::builder().layout_of::<u32>().build();

        // Fill the first slab.
        let mut first_slab_items = Vec::new();
        for _ in 0..DEFAULT_SLAB_CAPACITY.get() {
            first_slab_items.push(unsafe { pool.insert(1234_u32) });
        }

        // Fill the second slab.
        let mut second_slab_items = Vec::new();
        for _ in 0..DEFAULT_SLAB_CAPACITY.get() {
            second_slab_items.push(unsafe { pool.insert(5678_u32) });
        }

        // Remove the first item in the first slab to create a hole.
        let first_slab_first_item = first_slab_items.remove(0);
        pool.remove(first_slab_first_item);

        // Remove the first item in the second slab to create a hole.
        let second_slab_first_item = second_slab_items.remove(0);
        pool.remove(second_slab_first_item);

        // This will fill the hole in the first slab instead of allocating a new slab.
        let pooled_filled = unsafe { pool.insert(91011_u32) };

        assert_eq!(pooled_filled.coordinates.slab_index, 0);
        assert_eq!(pooled_filled.coordinates.index_in_slab, 0);
        unsafe {
            assert_eq!(pooled_filled.ptr().read(), 91011);
        }

        // Clean up remaining items.
        for item in first_slab_items {
            pool.remove(item);
        }
        for item in second_slab_items {
            pool.remove(item);
        }
        pool.remove(pooled_filled);
    }

    #[test]
    fn fill_first_hole_descending() {
        // If two slabs have a hole, we always fill a hole in the first (index-wise) slab.
        // We do not care which hole we fill (there may be multiple per slab), we just care
        // about which slab it is in.
        //
        // We create the holes in descending order (second slab first, then first slab).

        let mut pool = OpaquePool::builder().layout_of::<u32>().build();

        // Fill the first slab.
        let mut first_slab_items = Vec::new();
        for _ in 0..DEFAULT_SLAB_CAPACITY.get() {
            first_slab_items.push(unsafe { pool.insert(1234_u32) });
        }

        // Fill the second slab.
        let mut second_slab_items = Vec::new();
        for _ in 0..DEFAULT_SLAB_CAPACITY.get() {
            second_slab_items.push(unsafe { pool.insert(5678_u32) });
        }

        // Remove the first item in the second slab to create a hole.
        let second_slab_first_item = second_slab_items.remove(0);
        pool.remove(second_slab_first_item);

        // Remove the first item in the first slab to create a hole.
        let first_slab_first_item = first_slab_items.remove(0);
        pool.remove(first_slab_first_item);

        // This will fill the hole in the first slab instead of allocating a new slab.
        let pooled_filled = unsafe { pool.insert(91011_u32) };

        assert_eq!(pooled_filled.coordinates.slab_index, 0);
        assert_eq!(pooled_filled.coordinates.index_in_slab, 0);
        unsafe {
            assert_eq!(pooled_filled.ptr().read(), 91011);
        }

        // Clean up remaining items.
        for item in first_slab_items {
            pool.remove(item);
        }
        for item in second_slab_items {
            pool.remove(item);
        }
        pool.remove(pooled_filled);
    }

    #[test]
    fn shrink_to_fit_removes_empty_slabs() {
        let layout = Layout::new::<u32>();
        let mut pool = OpaquePool::builder().layout(layout).build();

        // Insert enough items to create multiple slabs
        let mut pooled_items = Vec::new();
        for i in 0..(DEFAULT_SLAB_CAPACITY.get() * 3) {
            pooled_items.push(unsafe { pool.insert(i as u32) });
        }

        // Verify we have 3 slabs
        assert_eq!(pool.capacity(), DEFAULT_SLAB_CAPACITY.get() * 3);

        // Remove all items from the last two slabs, keeping the first slab full
        let remaining_items: Vec<_> = pooled_items.drain(DEFAULT_SLAB_CAPACITY.get()..).collect();
        for item in remaining_items {
            pool.remove(item);
        }

        // Capacity should still be 3 slabs
        assert_eq!(pool.capacity(), DEFAULT_SLAB_CAPACITY.get() * 3);

        // Shrink to fit should remove the empty slabs
        pool.shrink_to_fit();

        // Now capacity should be 1 slab
        assert_eq!(pool.capacity(), DEFAULT_SLAB_CAPACITY.get());

        // Verify the remaining items are still accessible
        for (i, pooled) in pooled_items
            .iter()
            .take(DEFAULT_SLAB_CAPACITY.get())
            .enumerate()
        {
            unsafe {
                assert_eq!(pooled.ptr().read(), i as u32);
            }
        }
    }

    #[test]
    fn shrink_to_fit_all_empty_slabs() {
        let layout = Layout::new::<u32>();
        let mut pool = OpaquePool::builder().layout(layout).build();

        // Insert items to create slabs
        let mut pooled_items = Vec::new();
        for i in 0..(DEFAULT_SLAB_CAPACITY.get() * 2) {
            pooled_items.push(unsafe { pool.insert(i as u32) });
        }

        // Verify we have 2 slabs
        assert_eq!(pool.capacity(), DEFAULT_SLAB_CAPACITY.get() * 2);

        // Remove all items
        for item in pooled_items {
            pool.remove(item);
        }

        // Capacity should still be 2 slabs
        assert_eq!(pool.capacity(), DEFAULT_SLAB_CAPACITY.get() * 2);

        // Shrink to fit should remove all slabs
        pool.shrink_to_fit();

        // Now capacity should be 0
        assert_eq!(pool.capacity(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    #[allow(clippy::cast_possible_truncation, reason = "test values are small")]
    fn shrink_to_fit_no_empty_slabs() {
        let layout = Layout::new::<u32>();
        let mut pool = OpaquePool::builder().layout(layout).build();

        // Insert items to fill slabs completely
        let mut pooled_items = Vec::new();
        for i in 0..(DEFAULT_SLAB_CAPACITY.get() * 2) {
            pooled_items.push(unsafe { pool.insert(i as u32) });
        }

        let original_capacity = pool.capacity();

        // Shrink to fit should not change anything since no slabs are empty
        pool.shrink_to_fit();

        assert_eq!(pool.capacity(), original_capacity);

        // Verify all items are still accessible
        for (i, pooled) in pooled_items.iter().enumerate() {
            unsafe {
                assert_eq!(pooled.ptr().read(), i as u32);
            }
        }
    }

    #[test]
    fn shrink_to_fit_empty_pool() {
        let layout = Layout::new::<u32>();
        let mut pool = OpaquePool::builder().layout(layout).build();

        // Pool starts empty
        assert_eq!(pool.capacity(), 0);

        // Shrink to fit should not change anything
        pool.shrink_to_fit();

        assert_eq!(pool.capacity(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn shrink_then_grow_allocates_new_slab() {
        let mut pool = OpaquePool::builder().layout_of::<u32>().build();

        // Fill one complete slab
        let mut pooled_items = Vec::new();
        for i in 0..DEFAULT_SLAB_CAPACITY.get() {
            pooled_items.push(unsafe { pool.insert(i as u32) });
        }

        // Add one item to the second slab
        let overflow_item = unsafe { pool.insert(9999_u32) };

        // Verify we have 2 slabs
        assert_eq!(pool.slabs.len(), 2);
        assert_eq!(pool.capacity(), DEFAULT_SLAB_CAPACITY.get() * 2);

        // Remove the overflow item (making the second slab empty)
        pool.remove(overflow_item);

        // Shrink to fit should remove the empty second slab
        pool.shrink_to_fit();

        // Verify we're back to 1 slab
        assert_eq!(pool.slabs.len(), 1);
        assert_eq!(pool.capacity(), DEFAULT_SLAB_CAPACITY.get());
        assert!(pool.slabs[0].is_full());

        // Insert a new item - this should allocate a new slab since the existing one is full
        let new_item = unsafe { pool.insert(8888_u32) };

        // Verify we now have 2 slabs again
        assert_eq!(pool.slabs.len(), 2);
        assert_eq!(pool.capacity(), DEFAULT_SLAB_CAPACITY.get() * 2);

        // Verify the new item went to the second slab
        assert_eq!(new_item.coordinates.slab_index, 1);

        // Verify the new item is accessible
        unsafe {
            assert_eq!(new_item.ptr().read(), 8888);
        }

        // Clean up
        for item in pooled_items {
            pool.remove(item);
        }
        pool.remove(new_item);
    }

    #[test]
    fn reserve_increases_capacity() {
        let mut pool = OpaquePool::builder().layout_of::<u32>().build();

        // Initially no capacity
        assert_eq!(pool.capacity(), 0);

        // Reserve space for 10 items
        pool.reserve(10);
        assert!(pool.capacity() >= 10);

        // Insert an item - should not need to allocate more capacity
        let initial_capacity = pool.capacity();
        let pooled = unsafe { pool.insert(42_u32) };
        assert_eq!(pool.capacity(), initial_capacity);

        pool.remove(pooled);
    }

    #[test]
    fn reserve_with_existing_items() {
        let mut pool = OpaquePool::builder().layout_of::<u32>().build();

        // Insert some items first
        let pooled1 = unsafe { pool.insert(1_u32) };
        let pooled2 = unsafe { pool.insert(2_u32) };
        let current_len = pool.len();

        // Reserve additional space
        pool.reserve(5);
        assert!(pool.capacity() >= current_len + 5);

        // Verify existing items are still accessible
        unsafe {
            assert_eq!(pooled1.ptr().read(), 1);
            assert_eq!(pooled2.ptr().read(), 2);
        }

        pool.remove(pooled1);
        pool.remove(pooled2);
    }

    #[test]
    fn reserve_zero_does_nothing() {
        let mut pool = OpaquePool::builder().layout_of::<u32>().build();
        let initial_capacity = pool.capacity();

        pool.reserve(0);
        assert_eq!(pool.capacity(), initial_capacity);
    }

    #[test]
    fn reserve_with_sufficient_capacity_does_nothing() {
        let mut pool = OpaquePool::builder().layout_of::<u32>().build();

        // Reserve initial capacity
        pool.reserve(10);
        let capacity_after_reserve = pool.capacity();

        // Try to reserve less than what we already have
        pool.reserve(5);
        assert_eq!(pool.capacity(), capacity_after_reserve);
    }

    #[test]
    fn reserve_large_capacity() {
        let mut pool = OpaquePool::builder().layout_of::<u32>().build();

        // Reserve capacity for multiple slabs
        let large_count = DEFAULT_SLAB_CAPACITY.get() * 3 + 50;
        pool.reserve(large_count);
        assert!(pool.capacity() >= large_count);

        // Verify we can actually insert that many items
        let mut pooled_items = Vec::new();
        for i in 0..large_count {
            pooled_items.push(unsafe { pool.insert(i as u32) });
        }

        // Verify all items are accessible
        for (i, pooled) in pooled_items.iter().enumerate() {
            unsafe {
                assert_eq!(pooled.ptr().read(), i as u32);
            }
        }

        // Clean up
        for pooled in pooled_items {
            pool.remove(pooled);
        }
    }

    #[test]
    #[should_panic(expected = "capacity overflow")]
    fn reserve_overflow_panics() {
        let mut pool = OpaquePool::builder().layout_of::<u32>().build();

        // Insert one item to make len() = 1
        let _key = unsafe { pool.insert(42_u32) };

        // Try to reserve usize::MAX more items. Since len() = 1,
        // this will cause 1 + usize::MAX to overflow during capacity calculation
        pool.reserve(usize::MAX);
    }

    #[test]
    fn trait_object_usage() {
        // Define a trait for testing.
        trait Describable {
            fn describe(&self) -> String;
        }

        #[derive(Debug)]
        struct Product {
            name: String,
            price: f64,
        }

        impl Describable for Product {
            fn describe(&self) -> String {
                format!("Product: {} (${:.2})", self.name, self.price)
            }
        }

        let mut pool = OpaquePool::builder().layout_of::<Product>().build();

        // Insert a concrete type.
        let product = Product {
            name: "Widget".to_string(),
            price: 19.99,
        };

        // SAFETY: Product matches the layout used to create the pool.
        let pooled = unsafe { pool.insert(product) };

        // Create a reference from the pointer and use it as a trait object.
        unsafe {
            // SAFETY: The pointer is valid and points to a Product that we just inserted.
            let product_ref: &Product = pooled.ptr().as_ref();
            let trait_obj: &dyn Describable = product_ref;
            assert_eq!(trait_obj.describe(), "Product: Widget ($19.99)");
        }

        pool.remove(pooled);
    }

    #[test]
    fn trait_object_with_mutable_references() {
        trait Adjustable {
            fn adjust_value(&mut self, delta: i32);
            fn get_value(&self) -> i32;
        }

        #[derive(Debug)]
        struct Counter {
            value: i32,
        }

        impl Adjustable for Counter {
            fn adjust_value(&mut self, delta: i32) {
                self.value += delta;
            }

            fn get_value(&self) -> i32 {
                self.value
            }
        }

        let mut pool = OpaquePool::builder().layout_of::<Counter>().build();

        let counter = Counter { value: 10 };

        // SAFETY: Counter matches the layout used to create the pool.
        let pooled = unsafe { pool.insert(counter) };

        // Test mutable trait object.
        unsafe {
            // SAFETY: The pointer is valid and points to a Counter that we just inserted.
            let counter_ref: &mut Counter = pooled.ptr().as_mut();
            let trait_obj: &mut dyn Adjustable = counter_ref;

            assert_eq!(trait_obj.get_value(), 10);
            trait_obj.adjust_value(5);
            assert_eq!(trait_obj.get_value(), 15);
        }

        // Verify the change persisted.
        unsafe {
            // SAFETY: The pointer is valid and points to the same Counter.
            let counter_ref: &Counter = pooled.ptr().as_ref();
            assert_eq!(counter_ref.value, 15);
        }

        pool.remove(pooled);
    }
}
