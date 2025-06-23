use std::alloc::{Layout, alloc, dealloc};
use std::ptr::NonNull;
use std::{mem, ptr};

/// The result of inserting memory into a [`MemorySlab`].
///
/// Contains both the stable index for later operations and a pointer to the allocated memory.
#[derive(Debug)]
#[non_exhaustive]
pub struct MemorySlabInsertion {
    /// The stable index that can be used to retrieve or remove this memory later.
    index: usize,
    /// A pointer to the allocated memory block.
    ptr: NonNull<()>,
}

impl MemorySlabInsertion {
    /// Returns the stable index that can be used to retrieve or remove this memory later.
    #[must_use]
    pub fn index(&self) -> usize {
        self.index
    }

    /// Returns a pointer to the allocated memory block.
    #[must_use]
    pub fn ptr(&self) -> NonNull<()> {
        self.ptr
    }
}

/// Provides memory for `CAPACITY` objects with a specific layout without knowing their type.
///
/// A fixed-capacity heap-allocated collection that works with opaque memory blocks. Works similar
/// to a `Vec` but all items are located at stable addresses and the collection has a fixed
/// capacity of `CAPACITY` items, operating using an index for lookup. When you allocate memory,
/// you get back the index to use for accessing or deallocating the memory.
///
/// There are multiple ways to insert memory in the collection:
///
/// * [`insert()`][3] - allocates memory and returns a [`MemorySlabInsertion`] containing both the
///   index and a pointer to the memory. This provides both the stable index for later lookup and
///   immediate access to the memory.
///
/// # Out of band access
///
/// The collection does not create or keep references to the memory blocks, so it is valid to access
/// memory via pointers and to create custom references to memory from unsafe code even when not
/// holding an exclusive reference to the collection.
///
/// [1]: Self::get
/// [2]: Self::get
/// [3]: Self::insert
#[derive(Debug)]
pub struct MemorySlab<const CAPACITY: usize> {
    /// Layout of one item in the slab.
    layout: Layout,

    /// Offset to add to an `Entry` pointer to get to the actual data it represents.
    ///
    /// Essentially, each item in the slab is a combination of `Entry` and the actual data,
    /// pseudo-concatenated together in memory (respecting memory layout rules wrt padding).
    data_offset: usize,

    first_entry_ptr: NonNull<Entry>,

    /// Index of the next free slot in the collection. Think of this as a virtual stack of the most
    /// recently freed slots, with the stack entries stored in the collection entries themselves.
    /// Also known as intrusive freelist. This will point out of bounds if the collection is full.
    next_free_index: usize,

    /// The total number of items in the collection. This is not used by the collection itself but
    /// may be valuable to callers who want to know if the collection is empty because in many use
    /// cases the collection is the backing store for a custom allocation/pinning scheme for items
    /// used from unsafe code and may not be valid to drop when any items are still present.
    count: usize,
}

#[derive(Debug)]
enum Entry {
    Occupied,

    Vacant { next_free_index: usize },
}

impl<const CAPACITY: usize> MemorySlab<CAPACITY> {
    /// Creates a new slab with the specified memory layout.
    ///
    /// # Panics
    ///
    /// Panics if the slab would be zero-sized either due to capacity or item size being zero.
    #[must_use]
    pub fn new(layout: Layout) -> Self {
        assert!(CAPACITY > 0, "MemorySlab must have non-zero capacity");
        assert!(layout.size() > 0, "MemorySlab must have non-zero item size");
        assert!(
            CAPACITY < usize::MAX,
            "MemorySlab capacity must be less than usize::MAX"
        );

        // Calculate the combined layout for Entry + data
        let entry_layout = Layout::new::<Entry>();
        let (combined_layout, data_offset) = entry_layout
            .extend(layout)
            .expect("layout extension must be valid"); // Calculate the layout for the entire slab (array of combined layouts)
        let slab_layout = Layout::from_size_align(
            combined_layout
                .size()
                .checked_mul(CAPACITY)
                .expect("capacity overflow"),
            combined_layout.align(),
        )
        .expect("slab layout must be calculable");

        // SAFETY: The layout must be valid for the target allocation and not zero-sized
        // (guarded by assertion above).
        let ptr = NonNull::new(unsafe { alloc(slab_layout) })
            .expect("we do not intend to handle allocation failure as a real possibility - OOM is panic")
            .cast::<Entry>(); // Initialize all slots to `Vacant` to start with.
        for index in 0..CAPACITY {
            let offset = index
                .checked_mul(combined_layout.size())
                .expect("index overflow"); // SAFETY: We ensure in the layout calculation that there is enough space for all
            // items up to our indicated capacity. The offset is calculated safely above.
            // SAFETY: ptr is valid from allocation and offset is within bounds
            let entry_ptr = unsafe { ptr.as_ptr().cast::<u8>().add(offset) };
            // SAFETY: The pointer alignment is guaranteed by the layout calculation.
            #[allow(
                clippy::cast_ptr_alignment,
                reason = "layout calculation ensures proper alignment"
            )]
            let entry_ptr = entry_ptr.cast::<Entry>();

            // SAFETY: The pointer is valid for writes and of the right type, so all is well.
            unsafe {
                ptr::write(
                    entry_ptr,
                    Entry::Vacant {
                        next_free_index: index.checked_add(1).unwrap_or(CAPACITY),
                    },
                );
            }
        }

        Self {
            layout,
            data_offset,
            first_entry_ptr: ptr,
            next_free_index: 0,
            count: 0,
        }
    }

    #[must_use]
    fn combined_layout(&self) -> Layout {
        let entry_layout = Layout::new::<Entry>();
        entry_layout
            .extend(self.layout)
            .expect("layout extension must be valid")
            .0
    }

    #[must_use]
    fn slab_layout(&self) -> Layout {
        let combined_layout = self.combined_layout();
        Layout::from_size_align(
            combined_layout
                .size()
                .checked_mul(CAPACITY)
                .expect("capacity overflow"),
            combined_layout.align(),
        )
        .expect("slab layout must be calculable")
    }

    /// Returns the number of allocated memory blocks in the slab.
    #[must_use]
    pub fn len(&self) -> usize {
        self.count
    }

    /// Returns `true` if the slab contains no allocated memory blocks.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Returns `true` if the slab is at capacity and cannot allocate more memory blocks.
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.next_free_index >= CAPACITY
    }

    fn entry(&self, index: usize) -> &Entry {
        let entry_ptr = self.entry_ptr(index);

        // SAFETY: We ensured in the ctor that every entry is initialized and ensured above
        // that the pointer is valid, so we can safely dereference it.
        unsafe { entry_ptr.as_ref() }
    }

    #[expect(clippy::needless_pass_by_ref_mut, reason = "false positive")]
    fn entry_mut(&mut self, index: usize) -> &mut Entry {
        let mut entry_ptr = self.entry_ptr(index);

        // SAFETY: We ensured in the ctor that every entry is initialized and ensured above
        // that the pointer is valid, so we can safely dereference it.
        unsafe { entry_ptr.as_mut() }
    }

    fn entry_ptr(&self, index: usize) -> NonNull<Entry> {
        assert!(
            index < CAPACITY,
            "entry {index} index out of bounds in slab of capacity {CAPACITY}"
        );
        let combined_layout = self.combined_layout(); // Guarded by bounds check above, so we are guaranteed that the pointer is valid.
        // The arithmetic is checked to prevent overflow.
        let offset = index
            .checked_mul(combined_layout.size())
            .expect("offset calculation overflow");

        // SAFETY: The first_entry_ptr is valid from allocation, and offset is within bounds.
        let entry_ptr = unsafe { self.first_entry_ptr.as_ptr().cast::<u8>().add(offset) };

        // SAFETY: The pointer alignment is guaranteed by the layout calculation.
        #[allow(
            clippy::cast_ptr_alignment,
            reason = "layout calculation ensures proper alignment"
        )]
        let entry_ptr = entry_ptr.cast::<Entry>();

        // SAFETY: The entry_ptr is valid and non-null due to the allocation in `new()`.
        unsafe { NonNull::new_unchecked(entry_ptr) }
    }

    fn data_ptr(&self, index: usize) -> NonNull<()> {
        let entry_ptr = self.entry_ptr(index); // SAFETY: The data_offset is calculated correctly during construction to point to
        // the data portion of the entry+data combined layout.
        // SAFETY: entry_ptr is valid and data_offset is calculated correctly
        let data_ptr = unsafe { entry_ptr.as_ptr().cast::<u8>().add(self.data_offset) };

        // SAFETY: The data_ptr is valid and non-null due to the calculations above.
        unsafe { NonNull::new_unchecked(data_ptr.cast::<()>()) }
    }

    /// Returns a pointer to the memory at the specified index.
    ///
    /// # Panics
    ///
    /// Panics if the index is out of bounds or is not associated with allocated memory.
    #[must_use]
    pub fn get(&self, index: usize) -> NonNull<()> {
        match self.entry(index) {
            Entry::Occupied => self.data_ptr(index),
            Entry::Vacant { .. } => panic!(
                "attempted to get unallocated memory at index {index} in slab of capacity {CAPACITY}"
            ),
        }
    }
    /// Allocates memory in the slab and returns both the index and a pointer to the memory.
    ///
    /// Returns a [`MemorySlabInsertion`] containing the stable index that can be used for later
    /// operations like [`get()`] and [`remove()`], and a pointer to the allocated memory.
    ///
    /// # Panics
    ///
    /// Panics if the collection is full.
    ///
    /// [`get()`]: Self::get
    /// [`remove()`]: Self::remove
    #[must_use]
    pub fn insert(&mut self) -> MemorySlabInsertion {
        #[cfg(debug_assertions)]
        self.integrity_check();

        assert!(
            !self.is_full(),
            "cannot insert into a full slab of capacity {CAPACITY}"
        );

        // Pop the next free index from the stack of free entries.
        let index = self.next_free_index;
        let mut entry_ptr = self.entry_ptr(index);

        // SAFETY: We are not allowed to perform operations on the slab that would create another
        // reference to the entry (because we hold an exclusive reference). We do not do that, and
        // the slab by design does not create/hold permanent references to its entries.
        let entry = unsafe { entry_ptr.as_mut() };

        let previous_entry = mem::replace(entry, Entry::Occupied);
        self.next_free_index = match previous_entry {
            Entry::Vacant { next_free_index } => next_free_index,
            Entry::Occupied => panic!(
                "entry {index} was not vacant when we inserted into it in slab of capacity {CAPACITY}"
            ),
        };

        let data_ptr = self.data_ptr(index);

        self.count = self.count.checked_add(1).expect("count overflow in slab");

        MemorySlabInsertion {
            index,
            ptr: data_ptr,
        }
    }

    /// # Panics
    ///
    /// Panics if the index is out of bounds or is not associated with allocated memory.
    pub fn remove(&mut self, index: usize) {
        let next_free_index = self.next_free_index;

        {
            let entry = self.entry_mut(index);
            if matches!(entry, Entry::Vacant { .. }) {
                panic!("remove({index}) entry was vacant in slab of capacity {CAPACITY}");
            }

            *entry = Entry::Vacant { next_free_index };
        }

        // Push the removed item's entry onto the free stack.
        self.next_free_index = index;

        self.count = self
            .count
            .checked_sub(1)
            .expect("we asserted above that the entry is occupied so count must be non-zero");
    }

    #[cfg_attr(test, mutants::skip)] // This is essentially test logic, mutation is meaningless.
    #[cfg(debug_assertions)]
    /// Performs an integrity check on the slab data structure.
    ///
    /// This method is only available in debug builds and is used for testing and validation.
    #[allow(
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects,
        reason = "integrity check needs array access"
    )]
    pub fn integrity_check(&self) {
        let mut observed_is_vacant: [Option<bool>; CAPACITY] = [None; CAPACITY];
        let mut observed_next_free_index: [Option<usize>; CAPACITY] = [None; CAPACITY];
        let mut observed_occupied_count: usize = 0;

        for index in 0..CAPACITY {
            match self.entry(index) {
                Entry::Occupied => {
                    observed_is_vacant[index] = Some(false);
                    observed_occupied_count += 1;
                }
                Entry::Vacant { next_free_index } => {
                    observed_is_vacant[index] = Some(true);
                    observed_next_free_index[index] = Some(*next_free_index);
                }
            }
        }

        assert!(
            matches!(
                observed_is_vacant.get(self.next_free_index),
                None | Some(Some(true))
            ),
            "self.next_free_index points to an occupied slot {} in slab of capacity {CAPACITY}",
            self.next_free_index,
        );

        assert!(
            self.count == observed_occupied_count,
            "self.count {} does not match the observed occupied count {} in slab of capacity {CAPACITY}",
            self.count,
            observed_occupied_count,
        ); // Verify that all vacant entries are valid.
        for index in 0..CAPACITY {
            if !observed_is_vacant[index].expect("we just populated this above") {
                continue;
            }

            let next_free_index = observed_next_free_index[index]
                .expect("we just populated this above for vacant entries");

            if next_free_index == CAPACITY {
                continue;
            }

            assert!(
                next_free_index <= CAPACITY,
                "vacant entry {index} has out-of-bounds next_free_index {next_free_index} in slab of capacity {CAPACITY}"
            );

            assert!(
                observed_is_vacant[next_free_index].expect("index is in bounds"),
                "vacant entry {index} points to occupied entry {next_free_index} in slab of capacity {CAPACITY}"
            );
        }
    }
}

impl<const CAPACITY: usize> Drop for MemorySlab<CAPACITY> {
    fn drop(&mut self) {
        // Set them all to `Vacant` to ensure any cleanup is done.
        for index in 0..CAPACITY {
            let entry = self.entry_mut(index);

            *entry = Entry::Vacant {
                next_free_index: CAPACITY,
            };
        }

        // SAFETY: The layout must match between alloc and dealloc. It does.
        unsafe {
            dealloc(self.first_entry_ptr.as_ptr().cast(), self.slab_layout());
        }
    }
}

// SAFETY: Yes, there are raw pointers involved here but nothing inherently non-thread-mobile
// about it, so the slab can move between threads.
unsafe impl<const CAPACITY: usize> Send for MemorySlab<CAPACITY> {}

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
        let mut slab = MemorySlab::<3>::new(layout);

        let insertion_a = slab.insert();
        let insertion_b = slab.insert();
        let insertion_c = slab.insert();

        // Write some values
        unsafe {
            insertion_a.ptr.cast::<u32>().as_ptr().write(42);
            insertion_b.ptr.cast::<u32>().as_ptr().write(43);
            insertion_c.ptr.cast::<u32>().as_ptr().write(44);
        }

        // Read them back
        unsafe {
            assert_eq!(insertion_a.ptr.cast::<u32>().as_ptr().read(), 42);
            assert_eq!(insertion_b.ptr.cast::<u32>().as_ptr().read(), 43);
            assert_eq!(insertion_c.ptr.cast::<u32>().as_ptr().read(), 44);
        }

        // Also verify get() works
        unsafe {
            assert_eq!(
                slab.get(insertion_a.index).cast::<u32>().as_ptr().read(),
                42
            );
            assert_eq!(
                slab.get(insertion_b.index).cast::<u32>().as_ptr().read(),
                43
            );
            assert_eq!(
                slab.get(insertion_c.index).cast::<u32>().as_ptr().read(),
                44
            );
        }

        assert_eq!(slab.len(), 3);

        slab.remove(insertion_b.index);

        assert_eq!(slab.len(), 2);

        let insertion_d = slab.insert();

        unsafe {
            insertion_d.ptr.cast::<u32>().as_ptr().write(45);
            assert_eq!(
                slab.get(insertion_a.index).cast::<u32>().as_ptr().read(),
                42
            );
            assert_eq!(
                slab.get(insertion_c.index).cast::<u32>().as_ptr().read(),
                44
            );
            assert_eq!(
                slab.get(insertion_d.index).cast::<u32>().as_ptr().read(),
                45
            );
        }

        assert!(slab.is_full());
    }
    #[test]
    #[should_panic]
    fn panic_when_full() {
        let layout = Layout::new::<u32>();
        let mut slab = MemorySlab::<3>::new(layout);

        _ = slab.insert();
        _ = slab.insert();
        _ = slab.insert();

        _ = slab.insert();
    }
    #[test]
    #[should_panic]
    fn panic_when_oob_get() {
        let layout = Layout::new::<u32>();
        let mut slab = MemorySlab::<3>::new(layout);

        _ = slab.insert();
        _ = slab.get(1234);
    }
    #[test]
    fn insert_returns_correct_index_and_pointer() {
        let layout = Layout::new::<u32>();
        let mut slab = MemorySlab::<3>::new(layout);

        // We expect that we insert items in order, from the start (0, 1, 2, ...).

        let insertion = slab.insert();
        assert_eq!(insertion.index(), 0);
        unsafe {
            insertion.ptr().cast::<u32>().as_ptr().write(10);
            assert_eq!(slab.get(0).cast::<u32>().as_ptr().read(), 10);
        }

        let insertion = slab.insert();
        assert_eq!(insertion.index(), 1);
        unsafe {
            insertion.ptr().cast::<u32>().as_ptr().write(11);
            assert_eq!(slab.get(1).cast::<u32>().as_ptr().read(), 11);
        }

        let insertion = slab.insert();
        assert_eq!(insertion.index(), 2);
        unsafe {
            insertion.ptr().cast::<u32>().as_ptr().write(12);
            assert_eq!(slab.get(2).cast::<u32>().as_ptr().read(), 12);
        }
    }
    #[test]
    fn remove_makes_room() {
        let layout = Layout::new::<u32>();
        let mut slab = MemorySlab::<3>::new(layout);

        let insertion_a = slab.insert();
        let insertion_b = slab.insert();
        let insertion_c = slab.insert();

        unsafe {
            insertion_a.ptr.cast::<u32>().as_ptr().write(42);
            insertion_b.ptr.cast::<u32>().as_ptr().write(43);
            insertion_c.ptr.cast::<u32>().as_ptr().write(44);
        }

        slab.remove(insertion_b.index);

        let insertion_d = slab.insert();
        unsafe {
            insertion_d.ptr.cast::<u32>().as_ptr().write(45);

            assert_eq!(
                slab.get(insertion_a.index).cast::<u32>().as_ptr().read(),
                42
            );
            assert_eq!(
                slab.get(insertion_c.index).cast::<u32>().as_ptr().read(),
                44
            );
            assert_eq!(
                slab.get(insertion_d.index).cast::<u32>().as_ptr().read(),
                45
            );
        }
    }
    #[test]
    #[should_panic]
    fn remove_vacant_panics() {
        let layout = Layout::new::<u32>();
        let mut slab = MemorySlab::<3>::new(layout);

        slab.remove(1);
    }

    #[test]
    #[should_panic]
    fn get_vacant_panics() {
        let layout = Layout::new::<u32>();
        let slab = MemorySlab::<3>::new(layout);

        _ = slab.get(1);
    }

    #[test]
    fn different_layouts() {
        // Test with different sized types
        let layout_u64 = Layout::new::<u64>();
        let mut slab_u64 = MemorySlab::<2>::new(layout_u64);
        let insertion = slab_u64.insert();
        unsafe {
            insertion
                .ptr()
                .cast::<u64>()
                .as_ptr()
                .write(0x1234567890ABCDEF);
            assert_eq!(
                insertion.ptr().cast::<u64>().as_ptr().read(),
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
        let mut slab_large = MemorySlab::<2>::new(layout_large);

        let insertion = slab_large.insert();
        unsafe {
            insertion
                .ptr()
                .cast::<LargeStruct>()
                .as_ptr()
                .write(LargeStruct {
                    a: 1,
                    b: 2,
                    c: 3,
                    d: 4,
                });
            let value = insertion.ptr().cast::<LargeStruct>().as_ptr().read();
            assert_eq!(value.a, 1);
            assert_eq!(value.b, 2);
            assert_eq!(value.c, 3);
            assert_eq!(value.d, 4);
        }
    }

    #[test]
    #[should_panic]
    fn zero_capacity_is_panic() {
        let layout = Layout::new::<usize>();
        drop(MemorySlab::<0>::new(layout));
    }

    #[test]
    #[should_panic]
    fn zero_size_layout_is_panic() {
        let layout = Layout::from_size_align(0, 1).unwrap();
        drop(MemorySlab::<3>::new(layout));
    }

    #[test]
    fn in_refcell_works_fine() {
        use std::cell::RefCell;

        let layout = Layout::new::<u32>();
        let slab = RefCell::new(MemorySlab::<3>::new(layout));

        {
            let mut slab = slab.borrow_mut();
            let insertion_a = slab.insert();
            let insertion_b = slab.insert();
            let insertion_c = slab.insert();

            unsafe {
                insertion_a.ptr.cast::<u32>().as_ptr().write(42);
                insertion_b.ptr.cast::<u32>().as_ptr().write(43);
                insertion_c.ptr.cast::<u32>().as_ptr().write(44);

                assert_eq!(
                    slab.get(insertion_a.index).cast::<u32>().as_ptr().read(),
                    42
                );
                assert_eq!(
                    slab.get(insertion_b.index).cast::<u32>().as_ptr().read(),
                    43
                );
                assert_eq!(
                    slab.get(insertion_c.index).cast::<u32>().as_ptr().read(),
                    44
                );
            }

            slab.remove(insertion_b.index);

            let insertion_d = slab.insert();

            unsafe {
                insertion_d.ptr.cast::<u32>().as_ptr().write(45);
                assert_eq!(
                    slab.get(insertion_a.index).cast::<u32>().as_ptr().read(),
                    42
                );
                assert_eq!(
                    slab.get(insertion_c.index).cast::<u32>().as_ptr().read(),
                    44
                );
                assert_eq!(
                    slab.get(insertion_d.index).cast::<u32>().as_ptr().read(),
                    45
                );
            }
        }

        {
            let slab = slab.borrow();
            unsafe {
                assert_eq!(slab.get(0).cast::<u32>().as_ptr().read(), 42);
            }
            assert!(slab.is_full());
        }
    }

    #[test]
    fn multithreaded_via_mutex() {
        use std::sync::{Arc, Mutex};
        use std::thread;

        let layout = Layout::new::<u32>();
        let slab = Arc::new(Mutex::new(MemorySlab::<3>::new(layout)));

        let a;
        let b;
        let c;

        {
            let mut slab = slab.lock().unwrap();
            let insertion_a = slab.insert();
            let insertion_b = slab.insert();
            let insertion_c = slab.insert();
            a = insertion_a.index;
            b = insertion_b.index;
            c = insertion_c.index;

            unsafe {
                insertion_a.ptr.cast::<u32>().as_ptr().write(42);
                insertion_b.ptr.cast::<u32>().as_ptr().write(43);
                insertion_c.ptr.cast::<u32>().as_ptr().write(44);
            }
        }

        let slab_clone = Arc::clone(&slab);
        let handle = thread::spawn(move || {
            let mut slab = slab_clone.lock().unwrap();

            slab.remove(b);

            let insertion_d = slab.insert();

            unsafe {
                insertion_d.ptr.cast::<u32>().as_ptr().write(45);
                assert_eq!(slab.get(a).cast::<u32>().as_ptr().read(), 42);
                assert_eq!(slab.get(c).cast::<u32>().as_ptr().read(), 44);
                assert_eq!(
                    slab.get(insertion_d.index).cast::<u32>().as_ptr().read(),
                    45
                );
            }
        });

        handle.join().unwrap();

        let slab = slab.lock().unwrap();
        assert!(slab.is_full());
    }

    #[test]
    fn insert_returns_correct_pointer() {
        let layout = Layout::new::<u64>();
        let mut slab = MemorySlab::<2>::new(layout);

        let insertion = slab.insert();

        // Verify the pointer works and points to the right location
        unsafe {
            insertion.ptr().cast::<u64>().as_ptr().write(0xDEADBEEF);
            assert_eq!(
                slab.get(insertion.index()).cast::<u64>().as_ptr().read(),
                0xDEADBEEF
            );
            assert_eq!(insertion.ptr().cast::<u64>().as_ptr().read(), 0xDEADBEEF);
        }
    }

    #[test]
    fn stress_test_repeated_insert_remove() {
        let layout = Layout::new::<usize>();
        let mut slab = MemorySlab::<10>::new(layout);

        // Fill the slab
        let mut indices = Vec::new();
        for i in 0..10 {
            let insertion = slab.insert();
            unsafe {
                insertion.ptr().cast::<usize>().as_ptr().write(i * 100);
            }
            indices.push(insertion.index());
        }

        assert!(slab.is_full());

        // Remove every other item
        for i in (0..10).step_by(2) {
            slab.remove(indices[i]);
        }

        assert_eq!(slab.len(), 5);

        // Fill again
        for i in (0..10).step_by(2) {
            let insertion = slab.insert();
            unsafe {
                insertion.ptr().cast::<usize>().as_ptr().write(i * 100 + 50);
            }
            indices[i] = insertion.index();
        }

        assert!(slab.is_full());

        // Verify all values are correct
        for i in 0..10 {
            let expected = if i % 2 == 0 { i * 100 + 50 } else { i * 100 };
            unsafe {
                assert_eq!(
                    slab.get(indices[i]).cast::<usize>().as_ptr().read(),
                    expected
                );
            }
        }
    }

    #[test]
    fn different_alignments() {
        // Test with a type that has specific alignment requirements
        #[repr(align(16))]
        struct AlignedStruct {
            data: [u8; 32],
        }

        let layout = Layout::new::<AlignedStruct>();
        let mut slab = MemorySlab::<2>::new(layout);

        let insertion = slab.insert();

        // Verify alignment
        assert_eq!(insertion.ptr().as_ptr() as usize % 16, 0);

        unsafe {
            let aligned_ptr = insertion.ptr().cast::<AlignedStruct>();
            aligned_ptr
                .as_ptr()
                .write(AlignedStruct { data: [0x42; 32] });
            let value = aligned_ptr.as_ptr().read();
            assert_eq!(value.data[0], 0x42);
            assert_eq!(value.data[31], 0x42);
        }
    }
}
