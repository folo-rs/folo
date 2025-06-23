use std::alloc::Layout;
use std::ptr::NonNull;

use num::Integer;

use crate::MemorySlab;

/// An object pool of unbounded size that provides type-erased memory allocation.
///
/// Similar to [`MemorySlab`] but with dynamic capacity growth. The pool manages multiple
/// slabs internally and automatically allocates new slabs when needed.
///
/// There are multiple ways to insert memory into the collection:
///
/// * [`insert()`][3] - allocates memory and returns both the key and a pointer to the allocated memory.
///   This is the primary way to allocate memory and provides both the stable key for later lookup and
///   immediate access to the memory.
///
/// The pool returns a key for each allocated memory block, with blocks being keyed by this.
///
/// # Out of band access
///
/// The collection does not create or keep references to the memory blocks, so it is valid to access
/// memory via pointers and to create custom references to memory from unsafe code even when not
/// holding an exclusive reference to the collection.
///
/// You can obtain pointers to the memory blocks via the `NonNull<()>` returned by the
/// [`get()`][1] method. These pointers are guaranteed to be valid until the memory is removed
/// from the collection or the collection itself is dropped.
///
/// # Resource usage
///
/// As of today, the collection never shrinks, though future versions may offer facilities to do so.
///
/// [1]: Self::get
/// [3]: Self::insert
#[derive(Debug)]
pub struct MemoryPool {
    /// The layout of memory blocks managed by this pool.
    layout: Layout,

    /// The slabs that provide the storage of the pool.
    /// We use a Vec here to allow for dynamic capacity growth.
    ///
    /// For now, we only grow this Vec but in theory, one could implement shrinking as well
    /// by removing empty slabs.
    slabs: Vec<MemorySlab<SLAB_CAPACITY>>, // Under Miri, we use a smaller slab capacity because Miri test runtime scales by memory usage.

    /// Lowest index of any slab that has a vacant slot, if known. We use this to avoid scanning
    /// the entire collection for vacant slots when inserting memory. This being `None` does not
    /// imply that there are no vacant slots, it just means we do not know what slab they are in.
    /// In other words, this is a cache, not the ground truth - we set it to `None` when we lose
    /// confidence that the data is still valid but when we have no need to look up the new value.
    slab_with_vacant_slot_index: Option<usize>,
}

/// A key that can be used to reference up a memory block in a [`MemoryPool`].
///
/// Keys may be reused by the pool after a memory block is removed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Key {
    index_in_pool: usize,
}

/// Today, we assemble the pool from memory slabs, each containing a fixed number of memory blocks.
///
/// In the future, we may choose to be smarter about this, e.g. choosing the slab size dynamically
/// based on the size of the memory layout in order to match a memory page size, or another similar criterion.
/// This is why the parameter is also not exposed in the public API - we may want to change how we
/// perform the memory layout in a future version.
#[cfg(not(miri))]
const SLAB_CAPACITY: usize = 128;

impl MemoryPool {
    /// Creates a new [`MemoryPool`] with the specified memory layout.
    ///
    /// # Panics
    ///
    /// Panics if the layout has zero size.
    #[must_use]
    pub fn new(layout: Layout) -> Self {
        assert!(
            layout.size() > 0,
            "MemoryPool must have non-zero memory block size"
        );

        Self {
            layout,
            slabs: Vec::new(),
            slab_with_vacant_slot_index: None,
        }
    }

    /// Returns the memory layout used by this pool.
    #[must_use]
    pub fn layout(&self) -> Layout {
        self.layout
    }

    /// The number of allocated memory blocks in the pool.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slabs.iter().map(MemorySlab::len).sum()
    }

    /// The number of memory blocks the pool can accommodate without additional resource allocation.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.slabs.len()
            .checked_mul(SLAB_CAPACITY)
            .expect("overflow here would mean the pool can hold more memory blocks than virtual memory can fit, which makes no sense - it would never grow that big")
    }

    /// Whether the pool is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slabs.iter().all(MemorySlab::is_empty)
    }

    /// Gets a pointer to a memory block in the pool by its key.
    ///
    /// # Panics
    ///
    /// Panics if the key is not associated with a memory block.
    #[must_use]
    pub fn get(&self, key: Key) -> NonNull<()> {
        let coordinates = MemoryBlockCoordinates::<SLAB_CAPACITY>::from_key(key);

        self.slabs
            .get(coordinates.slab_index)
            .map(|s| s.get(coordinates.index_in_slab))
            .expect("key was not associated with a memory block in the pool")
    }
    /// Allocates memory in the pool and returns both the key and a pointer to the memory.
    #[must_use]
    pub fn insert(&mut self) -> (Key, NonNull<()>) {
        let slab_index = self.index_of_slab_with_vacant_slot();
        let slab = self
            .slabs
            .get_mut(slab_index)
            .expect("we just verified that there is a slab with a vacant slot at this index");

        // We invalidate the "slab with vacant slot" cache here if this is the last vacant slot.
        let predicted_slab_filled_slots = slab.len()
            .checked_add(1)
            .expect("we cannot overflow because there is at least one free slot, so it means there must be room to increment");

        if predicted_slab_filled_slots == SLAB_CAPACITY {
            self.slab_with_vacant_slot_index = None;
        }

        let insertion = slab.insert();
        let key =
            MemoryBlockCoordinates::<SLAB_CAPACITY>::from_parts(slab_index, insertion.index())
                .to_key();

        (key, insertion.ptr())
    }

    /// # Panics
    ///
    /// Panics if the key is not associated with a memory block.
    pub fn remove(&mut self, key: Key) {
        let index = MemoryBlockCoordinates::<SLAB_CAPACITY>::from_key(key);

        let Some(slab) = self.slabs.get_mut(index.slab_index) else {
            panic!("key was not associated with a memory block in the pool")
        };

        slab.remove(index.index_in_slab);

        // There is now a vacant slot in this slab! We may want to remember this for fast inserts.
        // We try to remember the lowest index of a slab with a vacant slot, so we
        // fill the collection from the start (to enable easier shrinking later).
        if self
            .slab_with_vacant_slot_index
            .is_none_or(|current| current > index.slab_index)
        {
            self.slab_with_vacant_slot_index = Some(index.slab_index);
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
            self.slabs.push(MemorySlab::new(self.layout));

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

#[derive(Debug)]
struct MemoryBlockCoordinates<const SLAB_CAPACITY: usize> {
    slab_index: usize,
    index_in_slab: usize,
}

impl<const SLAB_CAPACITY: usize> MemoryBlockCoordinates<SLAB_CAPACITY> {
    #[must_use]
    fn from_parts(slab: usize, index_in_slab: usize) -> Self {
        Self {
            slab_index: slab,
            index_in_slab,
        }
    }

    #[must_use]
    fn from_key(key: Key) -> Self {
        let (slab_index, index_in_slab) = key.index_in_pool.div_rem(&SLAB_CAPACITY);

        Self {
            slab_index,
            index_in_slab,
        }
    }

    #[must_use]
    fn to_key(&self) -> Key {
        Key {
            index_in_pool: self.slab_index.checked_mul(SLAB_CAPACITY)
                .and_then(|x| x.checked_add(self.index_in_slab))
                .expect("key indicates a memory block beyond the range of virtual memory - impossible to reach this point from a valid history")
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
        let mut pool = MemoryPool::new(layout);

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());

        let (key_a, ptr_a) = pool.insert();
        let (key_b, ptr_b) = pool.insert();
        let (key_c, ptr_c) = pool.insert();

        assert_eq!(pool.len(), 3);
        assert!(!pool.is_empty());
        assert!(pool.capacity() >= 3);

        // Write some values
        unsafe {
            ptr_a.cast::<u32>().as_ptr().write(42);
            ptr_b.cast::<u32>().as_ptr().write(43);
            ptr_c.cast::<u32>().as_ptr().write(44);
        }

        // Read them back via get
        unsafe {
            assert_eq!(pool.get(key_a).cast::<u32>().as_ptr().read(), 42);
            assert_eq!(pool.get(key_b).cast::<u32>().as_ptr().read(), 43);
            assert_eq!(pool.get(key_c).cast::<u32>().as_ptr().read(), 44);
        }

        pool.remove(key_b);

        let (key_d, ptr_d) = pool.insert();

        unsafe {
            ptr_d.cast::<u32>().as_ptr().write(45);
            assert_eq!(pool.get(key_a).cast::<u32>().as_ptr().read(), 42);
            assert_eq!(pool.get(key_c).cast::<u32>().as_ptr().read(), 44);
            assert_eq!(pool.get(key_d).cast::<u32>().as_ptr().read(), 45);
        }
    }

    #[test]
    #[should_panic]
    fn panic_when_empty_oob_get() {
        let layout = Layout::new::<u32>();
        let pool = MemoryPool::new(layout);

        let fake_key = Key { index_in_pool: 0 };
        _ = pool.get(fake_key);
    }

    #[test]
    #[should_panic]
    fn remove_nonexistent_panics() {
        let layout = Layout::new::<u32>();
        let mut pool = MemoryPool::new(layout);

        let fake_key = Key { index_in_pool: 0 };
        pool.remove(fake_key);
    }
    #[test]
    fn inserter_works() {
        let layout = Layout::new::<u64>();
        let mut pool = MemoryPool::new(layout);

        let (key, ptr) = pool.insert();

        unsafe {
            ptr.cast::<u64>().as_ptr().write(0x1234567890ABCDEF);
            assert_eq!(
                pool.get(key).cast::<u64>().as_ptr().read(),
                0x1234567890ABCDEF
            );
        }
    }
    #[test]
    #[allow(
        clippy::cast_possible_truncation,
        reason = "test uses small values that fit in u32"
    )]
    fn multi_slab_growth() {
        let layout = Layout::new::<u32>();
        let mut pool = MemoryPool::new(layout);

        // Insert more items than a single slab can hold to test growth
        let mut keys = Vec::new();
        for i in 0..200 {
            let (key, ptr) = pool.insert();
            unsafe {
                ptr.cast::<u32>().as_ptr().write(i);
            }
            keys.push(key);
        }

        assert_eq!(pool.len(), 200);
        assert!(pool.capacity() >= 200);

        // Verify all values are still accessible
        for (i, &key) in keys.iter().enumerate() {
            unsafe {
                assert_eq!(pool.get(key).cast::<u32>().as_ptr().read(), i as u32);
            }
        }
    }
    #[test]
    fn different_layouts() {
        // Test with different sized types
        let layout_u64 = Layout::new::<u64>();
        let mut pool_u64 = MemoryPool::new(layout_u64);
        let (key, ptr) = pool_u64.insert();
        unsafe {
            ptr.cast::<u64>().as_ptr().write(0x1234567890ABCDEF);
            assert_eq!(
                pool_u64.get(key).cast::<u64>().as_ptr().read(),
                0x1234567890ABCDEF
            );
        }

        // Test with larger struct
        #[repr(C)]
        struct LargeStruct {
            a: u64,
            b: u64,
            c: u64,
            d: u64,
        }

        let layout_large = Layout::new::<LargeStruct>();
        let mut pool_large = MemoryPool::new(layout_large);

        let (key, ptr) = pool_large.insert();
        unsafe {
            ptr.cast::<LargeStruct>().as_ptr().write(LargeStruct {
                a: 1,
                b: 2,
                c: 3,
                d: 4,
            });
            let value = pool_large.get(key).cast::<LargeStruct>().as_ptr().read();
            assert_eq!(value.a, 1);
            assert_eq!(value.b, 2);
            assert_eq!(value.c, 3);
            assert_eq!(value.d, 4);
        }
    }

    #[test]
    #[should_panic]
    fn zero_size_layout_is_panic() {
        let layout = Layout::from_size_align(0, 1).unwrap();
        drop(MemoryPool::new(layout));
    }
    #[test]
    fn stress_test_repeated_insert_remove() {
        let layout = Layout::new::<usize>();
        let mut pool = MemoryPool::new(layout);

        // Insert and remove many items to test slab management
        for iteration in 0..10 {
            let mut keys = Vec::new();
            for i in 0..50 {
                let (key, ptr) = pool.insert();
                unsafe {
                    ptr.cast::<usize>().as_ptr().write(iteration * 100 + i);
                }
                keys.push(key);
            }

            // Remove every other item
            for i in (0..50).step_by(2) {
                pool.remove(keys[i]);
            }

            // Verify remaining items
            for i in (1..50).step_by(2) {
                unsafe {
                    assert_eq!(
                        pool.get(keys[i]).cast::<usize>().as_ptr().read(),
                        iteration * 100 + i
                    );
                }
            }

            // Remove remaining items
            for i in (1..50).step_by(2) {
                pool.remove(keys[i]);
            }
        }

        assert!(pool.is_empty());
    }
}
