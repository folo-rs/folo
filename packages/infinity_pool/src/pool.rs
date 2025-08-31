use std::alloc::Layout;
use std::mem::MaybeUninit;

use crate::{DropPolicy, PoolHandle, Slab, SlabLayout};

// TODO: Factor out the vacancy cache into its own type.

/// A pool of objects of uniform memory layout.
///
/// This is the building block for creating different kinds of object pools. Underneath, they all
/// place their items in this pool, which manages the memory capacity, places items into that
/// capacity and grants low-level access to the items.
///
/// The pool does not care what the type of the items is, only that the types all match the memory
/// layout specified at creation time. This is a safety requirement of the pool APIs, guaranteed
/// by the higher-level pool types that internally use this pool.
#[derive(Debug)]
pub(crate) struct Pool {
    /// The layout of each slab in the pool, determined based on the object
    /// layout provided at pool creation time.
    slab_layout: SlabLayout,

    /// The slabs that make up the pool's memory capacity. Automatically extended
    /// with new slabs as needed. Shrinking is supported but must be manually commanded.
    slabs: Vec<Slab>,

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

impl Pool {
    /// Creates a new pool for objects of the specified layout.
    ///
    /// # Panics
    ///
    /// Panics if the object layout has zero size.
    #[must_use]
    pub(crate) fn new(object_layout: Layout, drop_policy: DropPolicy) -> Self {
        let slab_layout = SlabLayout::new(object_layout);

        Self {
            slab_layout,
            slabs: Vec::new(),
            slab_with_vacant_slot_index: None,
            drop_policy,
            length: 0,
        }
    }

    /// Returns the layout of objects stored in this pool.
    #[must_use]
    pub(crate) fn object_layout(&self) -> Layout {
        self.slab_layout.object_layout()
    }

    /// Returns the number of objects currently in the pool.
    #[cfg_attr(test, mutants::skip)] // Can be mutated to infinitely growing memory use and/or infinite loop.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.length
    }

    /// Returns the total capacity across all slabs in the pool.
    #[cfg_attr(test, mutants::skip)] // Can be mutated to infinitely growing memory use and/or infinite loop.
    #[must_use]
    pub(crate) fn capacity(&self) -> usize {
        // Wrapping here would imply capacity is greater than virtual memory,
        // which is impossible because we can never create that many slabs.
        self.slabs
            .len()
            .wrapping_mul(self.slab_layout.capacity().get())
    }

    /// Returns `true` if the pool contains no objects.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Reserves capacity for at least `additional` more objects.
    pub(crate) fn reserve(&mut self, additional: usize) {
        let required_capacity = self
            .len()
            .checked_add(additional)
            .expect("requested capacity exceeds size of virtual memory");

        if self.capacity() >= required_capacity {
            return;
        }

        // Calculate how many additional slabs we need
        let current_slabs = self.slabs.len();
        let required_slabs = required_capacity.div_ceil(self.slab_layout.capacity().get());
        let additional_slabs = required_slabs.saturating_sub(current_slabs);

        for _ in 0..additional_slabs {
            self.slabs
                .push(Slab::new(self.slab_layout, self.drop_policy));
        }
    }

    /// Removes empty slabs to reduce memory usage.
    pub(crate) fn shrink_to_fit(&mut self) {
        // Find the last non-empty slab by scanning from the end
        let new_len = self
            .slabs
            .iter()
            .enumerate()
            .rev()
            .find_map(|(idx, slab)| {
                if !slab.is_empty() {
                    // Cannot wrap because that would imply we have more slabs than the size
                    // of virtual memory, which is impossible.
                    Some(idx.wrapping_add(1))
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

    /// Inserts an object into the pool and returns a handle to it.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the layout of `T` matches the pool's object layout.
    pub(crate) unsafe fn insert<T>(&mut self, value: T) -> PoolHandle<T> {
        // Implement insert() in terms of insert_with() to reduce logic duplication.
        // SAFETY: Forwarding safety requirements to the caller.
        unsafe {
            self.insert_with(|uninit: &mut MaybeUninit<T>| {
                uninit.write(value);
            })
        }
    }

    /// Inserts an object into the pool via closure and returns a handle to it.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the layout of `T` matches the pool's object layout.
    /// The closure must correctly initialize the object.
    pub(crate) unsafe fn insert_with<T, F>(&mut self, f: F) -> PoolHandle<T>
    where
        F: FnOnce(&mut MaybeUninit<T>),
    {
        debug_assert_eq!(
            Layout::new::<T>(),
            self.object_layout(),
            "T layout does not match pool's item layout"
        );

        let slab_index = self.index_of_slab_with_vacant_slot();

        #[expect(
            clippy::indexing_slicing,
            reason = "we just received knowledge that there is a slab with a vacant slot at this index"
        )]
        let slab = &mut self.slabs[slab_index];

        // We invalidate the "slab with vacant slot" cache here if this is the last vacant slot.
        //
        // We cannot overflow because there is at least one free slot,
        // which means there must be room to increment.
        let predicted_slab_filled_slots = slab.len().wrapping_add(1);

        if predicted_slab_filled_slots == self.slab_layout.capacity().get() {
            self.slab_with_vacant_slot_index = None;
        }

        // SAFETY: Forwarding guarantee from caller that T's layout matches the pool's layout
        // and that the closure properly initializes the value.
        let slab_handle = unsafe { slab.insert_with(f) };

        // Update our tracked length since we just inserted an item.
        // This can never overflow since that would mean the pool is greater than virtual memory.
        self.length = self.length.wrapping_add(1);

        // The pool itself does not care about the type T but for the convenience of the caller
        // we imbue the PoolHandle with the type information, to reduce required casting by caller.
        PoolHandle::new(slab_index, slab_handle)
    }

    /// Removes an object from the pool, dropping it.
    ///
    /// # Panics
    ///
    /// Panics if the handle does not reference an existing item in this pool.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the handle is valid and belongs to this pool.
    pub(crate) unsafe fn remove<T: ?Sized>(&mut self, handle: PoolHandle<T>) {
        let slab = self
            .slabs
            .get_mut(handle.slab_index())
            .expect("the PoolHandle did not point to an existing item in the pool");

        // In principle, we could return the value here if `T: Unpin` but there is no need
        // for this functionality at present, so we do not implement it to reduce complexity.
        // SAFETY: Forwarding guarantees from caller.
        unsafe {
            slab.remove(handle.slab_handle());
        }

        // Update our tracked length since we just removed an item.
        // This cannot wrap around because we just removed an item, so the value must be at least 1.
        self.length = self.length.wrapping_sub(1);

        // There is now a vacant slot in this slab! We may want to remember this for fast insertions.
        // We try to remember the lowest index of a slab with a vacant slot, so we
        // fill the collection from the start (to enable easier shrinking later).
        self.update_vacant_slot_cache(handle.slab_index());
    }

    /// Removes an object from the pool and returns it.
    ///
    /// # Panics
    ///
    /// Panics if the handle does not reference an existing item in this pool.
    ///
    /// Panics if `T` is a type-erased slab handle (`PoolHandle<()>`).
    ///
    ///
    /// # Safety
    ///
    /// The caller must ensure that the handle is valid and belongs to this pool.
    pub(crate) unsafe fn remove_unpin<T: Unpin>(&mut self, handle: PoolHandle<T>) -> T {
        // We would rather prefer to check for `PoolHandle<()>` specifically but
        // that would imply specialization or `T: 'static` for TypeId shenanigans.
        // This is good enough because type-erasing a handle is the only way to get a
        // handle to a ZST anyway because the slab does not even support ZSTs.
        assert_ne!(
            size_of::<T>(),
            0,
            "cannot remove_unpin() through a type-erased handle"
        );

        let slab = self
            .slabs
            .get_mut(handle.slab_index())
            .expect("the PoolHandle did not point to an existing item in the pool");

        // SAFETY: The PoolHandle<T> guarantees the type T is correct for this pool slot.
        let value = unsafe { slab.remove_unpin::<T>(handle.slab_handle()) };

        // Update our tracked length since we just removed an item.
        // This cannot wrap around because we just removed an item, so the value must be at least 1.
        self.length = self.length.wrapping_sub(1);

        // There is now a vacant slot in this slab! We may want to remember this for fast insertions.
        // We try to remember the lowest index of a slab with a vacant slot, so we
        // fill the collection from the start (to enable easier shrinking later).
        self.update_vacant_slot_cache(handle.slab_index());

        value
    }

    /// Adds a new slab to the pool and returns its index.
    #[must_use]
    fn add_new_slab(&mut self) -> usize {
        self.slabs
            .push(Slab::new(self.slab_layout, self.drop_policy));

        // This can never wrap around because we just added a slab, so len() is at least 1.
        self.slabs.len().wrapping_sub(1)
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
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    reason = "tests focus on succinct code and do not need to tick all the boxes"
)]
mod tests {
    use std::alloc::Layout;
    use std::mem::MaybeUninit;

    use super::*;

    #[test]
    fn new_pool_is_empty() {
        let pool = Pool::new(Layout::new::<u64>(), DropPolicy::MayDropItems);

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity(), 0);
        assert_eq!(pool.object_layout(), Layout::new::<u64>());
    }

    #[test]
    fn insert_and_length() {
        let mut pool = Pool::new(Layout::new::<u32>(), DropPolicy::MayDropItems);

        // SAFETY: u32 matches the layout used to create the pool
        let _handle1 = unsafe { pool.insert(42_u32) };
        assert_eq!(pool.len(), 1);
        assert!(!pool.is_empty());

        // SAFETY: u32 matches the layout used to create the pool
        let _handle2 = unsafe { pool.insert(100_u32) };
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn capacity_grows_with_slabs() {
        let mut pool = Pool::new(Layout::new::<u64>(), DropPolicy::MayDropItems);

        assert_eq!(pool.capacity(), 0);

        // SAFETY: u64 matches the layout used to create the pool
        let _handle = unsafe { pool.insert(123_u64) };

        // Should have at least one slab's worth of capacity now
        assert!(pool.capacity() > 0);
        let initial_capacity = pool.capacity();

        // Fill up the slab to force creation of a new one
        for i in 1..initial_capacity {
            // SAFETY: u64 matches the layout used to create the pool
            let _handle = unsafe { pool.insert(i as u64) };
        }

        // One more insert should create a new slab
        // SAFETY: u64 matches the layout used to create the pool
        let _handle = unsafe { pool.insert(999_u64) };

        assert!(pool.capacity() >= initial_capacity * 2);
    }

    #[test]
    fn reserve_creates_capacity() {
        let mut pool = Pool::new(Layout::new::<u8>(), DropPolicy::MayDropItems);

        pool.reserve(100);
        assert!(pool.capacity() >= 100);

        let initial_capacity = pool.capacity();
        pool.reserve(50); // Should not increase capacity
        assert_eq!(pool.capacity(), initial_capacity);

        pool.reserve(200); // Should increase capacity
        assert!(pool.capacity() >= 200);
    }

    #[test]
    fn insert_with_closure() {
        let mut pool = Pool::new(Layout::new::<u64>(), DropPolicy::MayDropItems);

        // SAFETY: u64 matches the layout used to create the pool
        let handle = unsafe {
            pool.insert_with(|uninit: &mut MaybeUninit<u64>| {
                uninit.write(42);
            })
        };

        assert_eq!(pool.len(), 1);

        // SAFETY: Handle is valid and comes from this pool
        let value = unsafe { pool.remove_unpin(handle) };
        assert_eq!(value, 42);
    }

    #[test]
    fn remove_decreases_length() {
        let mut pool = Pool::new(Layout::new::<String>(), DropPolicy::MayDropItems);

        // SAFETY: String matches the layout used to create the pool
        let handle1 = unsafe { pool.insert("hello".to_string()) };
        // SAFETY: String matches the layout used to create the pool
        let handle2 = unsafe { pool.insert("world".to_string()) };

        assert_eq!(pool.len(), 2);

        // SAFETY: Handle is valid and comes from this pool
        unsafe {
            pool.remove(handle1);
        }
        assert_eq!(pool.len(), 1);

        // SAFETY: Handle is valid and comes from this pool
        unsafe {
            pool.remove(handle2);
        }
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn remove_unpin_returns_value() {
        let mut pool = Pool::new(Layout::new::<i32>(), DropPolicy::MayDropItems);

        // SAFETY: i32 matches the layout used to create the pool
        let handle = unsafe { pool.insert(-456_i32) };

        // SAFETY: Handle is valid and comes from this pool
        let value = unsafe { pool.remove_unpin(handle) };
        assert_eq!(value, -456);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn shrink_to_fit_removes_empty_slabs() {
        let mut pool = Pool::new(Layout::new::<u8>(), DropPolicy::MayDropItems);

        // Force creation of multiple slabs
        pool.reserve(300);
        let initial_capacity = pool.capacity();

        // Add some items
        let mut handles = Vec::new();
        for i in 0..10 {
            // SAFETY: u8 matches the layout used to create the pool
            handles.push(unsafe { pool.insert(u8::try_from(i).unwrap()) });
        }

        // Remove all items
        for handle in handles {
            // SAFETY: Handle is valid and comes from this pool
            unsafe {
                pool.remove(handle);
            }
        }

        assert!(pool.is_empty());

        // Shrink should reduce capacity but may not eliminate all empty slabs
        // (only trailing empty slabs are removed)
        pool.shrink_to_fit();

        // Capacity should be reduced or at least not increased
        assert!(pool.capacity() <= initial_capacity);
    }

    #[test]
    fn handle_provides_access_to_object() {
        let mut pool = Pool::new(Layout::new::<u64>(), DropPolicy::MayDropItems);

        // SAFETY: u64 matches the layout used to create the pool
        let handle = unsafe { pool.insert(12345_u64) };

        // Access the value through the handle's pointer
        let ptr = handle.ptr();
        // SAFETY: Handle is valid and points to a u64
        let value = unsafe { ptr.as_ref() };
        assert_eq!(*value, 12345);
    }

    #[test]
    fn handles_are_copyable() {
        let mut pool = Pool::new(Layout::new::<u32>(), DropPolicy::MayDropItems);

        // SAFETY: u32 matches the layout used to create the pool
        let handle1 = unsafe { pool.insert(789_u32) };
        let handle2 = handle1;
        #[expect(clippy::clone_on_copy, reason = "intentional, testing cloning")]
        let handle3 = handle1.clone();

        assert_eq!(handle1, handle2);
        assert_eq!(handle1, handle3);
        assert_eq!(handle2, handle3);
    }

    #[test]
    fn multiple_removals_and_insertions() {
        let mut pool = Pool::new(Layout::new::<usize>(), DropPolicy::MayDropItems);

        // Insert, remove, insert again to test slot reuse
        // SAFETY: usize matches the layout used to create the pool
        let handle1 = unsafe { pool.insert(1_usize) };
        // SAFETY: Handle is valid and comes from this pool
        unsafe {
            pool.remove(handle1);
        }

        // SAFETY: usize matches the layout used to create the pool
        let handle2 = unsafe { pool.insert(2_usize) };

        assert_eq!(pool.len(), 1);

        // SAFETY: Handle is valid and comes from this pool
        let value = unsafe { pool.remove_unpin(handle2) };
        assert_eq!(value, 2);
    }
}
