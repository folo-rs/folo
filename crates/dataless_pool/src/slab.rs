use std::alloc::{Layout, alloc, dealloc};
use std::num::NonZero;
use std::ptr::NonNull;
use std::{mem, ptr, thread};

/// Provides memory for a specified number of objects with a specific layout without
/// knowing their type.
///
/// A fixed-capacity heap-allocated collection that works with opaque memory blocks.
/// Works similar to a `Vec` but only reserves memory, without placing any data in the
/// reserved memory capacity. All items are located at stable addresses and the collection
/// has a fixed capacity determined at construction time, operating using an index for
/// lookup. When you reserve memory, you get back the index to use for accessing or
/// deallocating the memory.
///
/// # Out of band access
///
/// The collection does not create or keep references to the memory blocks, so it is valid to access
/// memory via pointers and to create custom references to memory from unsafe code even when not
/// holding an exclusive reference to the collection.
#[derive(Debug)]
pub(crate) struct DatalessSlab {
    capacity: NonZero<usize>,

    layout_info: SlabLayoutInfo,

    /// This points to the beginning of the allocated memory block that contains all entries.
    /// Each entry is a combination of `EntryMeta` and the actual item data, with padding.
    first_entry_meta_ptr: NonNull<EntryMeta>,

    /// Index of the next free slot in the collection. Think of this as a virtual stack of the most
    /// recently freed slots, with the stack entries stored in the collection entries themselves.
    /// Also known as intrusive freelist. This will point out of bounds if the collection is full.
    next_free_index: usize,

    /// Enables callers to detect if any items are still present when dropping the backing store,
    /// which is critical for unsafe code that may have pointers to items in the collection.
    count: usize,
}

/// The result of reserving a memory block in a [`DatalessSlab`].
#[derive(Debug)]
pub(crate) struct SlabReservation {
    index: usize,

    ptr: NonNull<()>,
}

impl SlabReservation {
    #[must_use]
    pub(crate) fn index(&self) -> usize {
        self.index
    }

    /// Returns a pointer to the reserved memory block.
    #[must_use]
    pub(crate) fn ptr(&self) -> NonNull<()> {
        self.ptr
    }
}

/// Layout calculations for a [`DatalessSlab`].
#[derive(Clone, Debug, Eq, PartialEq)]
struct SlabLayoutInfo {
    /// Layout of the `EntryMeta` and the item it owns combined, padded to alignment.
    /// The size of this layout represents the stride between consecutive entries.
    combined_entry_layout: Layout,

    /// Offset to add to an `EntryMeta` pointer to get to the actual item inside the entry.
    item_offset: usize,

    /// Layout of the entire slab (array of all the entries for the given capacity).
    entry_array_layout: Layout,
}

impl SlabLayoutInfo {
    /// Calculates layout information for a slab with the given item layout and capacity.
    ///
    /// # Panics
    ///
    /// Panics if the item layout has zero size or if layout calculations overflow.
    #[must_use]
    fn calculate(item_layout: Layout, capacity: NonZero<usize>) -> Self {
        assert!(
            item_layout.size() > 0,
            "SlabLayoutInfo cannot be calculated for zero-sized item layout"
        );

        // Calculate the combined layout for Entry + item.
        let meta_layout = Layout::new::<EntryMeta>();

        let (combined_entry_layout, item_offset) = meta_layout
            .extend(item_layout)
            .expect("layout extension cannot fail for valid layouts with reasonable sizes");

        // Calculate the layout for the entire slab (array of combined layouts).
        // Layout::pad_to_align() ensures the size is a multiple of alignment,
        // which is exactly what we need for proper array element spacing.
        let combined_entry_layout = combined_entry_layout.pad_to_align();

        let total_size = combined_entry_layout
            .size()
            .checked_mul(capacity.get())
            .expect("total size calculation cannot overflow for reasonable capacity values");

        let slab_layout = Layout::from_size_align(total_size, combined_entry_layout.align())
            .expect("slab layout calculation cannot fail for valid combined layouts");

        Self {
            combined_entry_layout,
            item_offset,
            entry_array_layout: slab_layout,
        }
    }
}

#[derive(Debug)]
enum EntryMeta {
    /// We do not know the type of the item, so we cannot name the item here. Instead,
    /// the item will follow the entry at `item_offset` bytes from the start of the entry.
    Occupied,

    Vacant {
        next_free_index: usize,
    },
}

impl DatalessSlab {
    /// Creates a new slab with the specified item memory layout and capacity.
    ///
    /// # Panics
    ///
    /// Panics if the slab would be zero-sized due to item size being zero.
    #[must_use]
    pub(crate) fn new(item_layout: Layout, capacity: NonZero<usize>) -> Self {
        let layout_info = SlabLayoutInfo::calculate(item_layout, capacity);

        // SAFETY: The layout is valid for the target allocation and not zero-sized.
        // (guarded by assertions above).
        let first_entry_ptr = NonNull::new(unsafe { alloc(layout_info.entry_array_layout) })
            .expect("we do not intend to handle allocation failure as a real possibility - OOM results in panic")
            .cast::<EntryMeta>();

        // Initialize all slots to `Vacant` to start with.
        for index in 0_usize..capacity.get() {
            let offset = index
                .checked_mul(layout_info.combined_entry_layout.size())
                .expect("index offset calculation cannot overflow for reasonable index values");

            // SAFETY: We allocated enough space for all items up to the indicated capacity.
            // and the offset is calculated safely above.
            let entry_meta_ptr = unsafe { first_entry_ptr.as_ptr().cast::<u8>().add(offset) };

            // SAFETY: The pointer alignment is guaranteed by Layout::pad_to_align() which ensures
            // the stride (element size) is a multiple of the alignment, providing proper spacing
            // between array elements.
            #[allow(
                clippy::cast_ptr_alignment,
                reason = "Layout::pad_to_align() ensures proper alignment between array elements"
            )]
            let entry_meta_ptr = entry_meta_ptr.cast::<EntryMeta>();

            // SAFETY: The pointer is valid for writes and points to properly allocated memory.
            unsafe {
                ptr::write(
                    entry_meta_ptr,
                    EntryMeta::Vacant {
                        next_free_index: index.checked_add(1_usize).expect(
                            "requires the container to be larger than virtual memory - impossible",
                        ),
                    },
                );
            }
        }

        Self {
            capacity,
            layout_info,
            first_entry_meta_ptr: first_entry_ptr,
            next_free_index: 0,
            count: 0,
        }
    }

    /// Returns the number of reserved memory blocks in the slab.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.count
    }

    /// Returns `true` if the slab contains no reserved memory blocks.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Returns `true` if the slab is at capacity and cannot reserve more memory blocks.
    #[must_use]
    pub(crate) fn is_full(&self) -> bool {
        self.next_free_index >= self.capacity.get()
    }

    fn entry_meta(&self, index: usize) -> &EntryMeta {
        let entry_meta_ptr = self.entry_meta_ptr(index);

        // SAFETY: We ensured in the ctor that every entry is initialized and ensured above
        // that the pointer is valid, so we can safely dereference it.
        unsafe { entry_meta_ptr.as_ref() }
    }

    #[expect(clippy::needless_pass_by_ref_mut, reason = "false positive")]
    fn entry_meta_mut(&mut self, index: usize) -> &mut EntryMeta {
        let mut entry_meta_ptr = self.entry_meta_ptr(index);

        // SAFETY: We ensured in the ctor that every entry is initialized and ensured above
        // that the pointer is valid, so we can safely dereference it.
        unsafe { entry_meta_ptr.as_mut() }
    }

    fn entry_meta_ptr(&self, index: usize) -> NonNull<EntryMeta> {
        assert!(
            index < self.capacity.get(),
            "entry {index} index out of bounds in slab of capacity {}",
            self.capacity.get()
        );

        // Guarded by bounds check above, so we are guaranteed that the pointer is valid.
        // The arithmetic is checked to prevent overflow.
        let offset = index
            .checked_mul(self.layout_info.combined_entry_layout.size())
            .expect("offset calculation cannot overflow for valid index values as that would exceed virtual memory limits");

        // SAFETY: The first_entry_ptr is valid from allocation, and offset is within bounds.
        // The pointer alignment is guaranteed by Layout::pad_to_align() which ensures
        // the entry layout is a multiple of the alignment, providing proper spacing
        // between array elements.
        let entry_meta_ptr = unsafe { self.first_entry_meta_ptr.as_ptr().byte_add(offset) };

        // SAFETY: The entry_ptr is valid and non-null due to the allocation in `new()`.
        unsafe { NonNull::new_unchecked(entry_meta_ptr) }
    }

    fn item_ptr(&self, index: usize) -> NonNull<()> {
        let entry_meta_ptr = self.entry_meta_ptr(index);

        // SAFETY: entry_meta_ptr is valid and item_offset is calculated correctly.
        let item_ptr = unsafe {
            entry_meta_ptr
                .as_ptr()
                .byte_add(self.layout_info.item_offset)
        };

        // SAFETY: The ptr is valid and non-null due to the calculations above.
        unsafe { NonNull::new_unchecked(item_ptr.cast::<()>()) }
    }

    /// Reserves memory in the slab and returns both the index and a pointer to the memory.
    ///
    /// Returns a [`SlabReservation`] containing the stable index that can be used for later
    /// operations like [`get()`] and [`release()`], and a pointer to the reserved memory.
    ///
    /// # Panics
    ///
    /// Panics if the collection is full.
    ///
    /// [`get()`]: Self::get
    /// [`release()`]: Self::release
    #[must_use]
    pub(crate) fn reserve(&mut self) -> SlabReservation {
        #[cfg(debug_assertions)]
        self.integrity_check();

        assert!(
            !self.is_full(),
            "cannot reserve memory in a full slab of capacity {}",
            self.capacity.get()
        );

        // Pop the next free index from the stack of free entries.
        let index = self.next_free_index;
        let mut entry_meta_ptr = self.entry_meta_ptr(index);

        // SAFETY: We are not allowed to perform operations on the slab that would create another
        // reference to the entry (because we hold an exclusive reference). We do not do that, and
        // the slab by design does not create/hold permanent references to its entries.
        let entry_meta_ptr = unsafe { entry_meta_ptr.as_mut() };

        let previous_entry = mem::replace(entry_meta_ptr, EntryMeta::Occupied);
        self.next_free_index = match previous_entry {
            EntryMeta::Vacant { next_free_index } => next_free_index,
            EntryMeta::Occupied => panic!(
                "entry {index} was not vacant when we reserved it in slab of capacity {}",
                self.capacity.get()
            ),
        };

        let item_ptr = self.item_ptr(index);

        self.count = self
            .count
            .checked_add(1)
            .expect("count cannot overflow because it is bounded by capacity which is bounded by usize::MAX");

        SlabReservation {
            index,
            ptr: item_ptr,
        }
    }

    /// Releases memory that was previously reserved at the given index.
    ///
    /// # Panics
    ///
    /// Panics if the index is out of bounds or is not associated with reserved memory.
    ///
    /// # Safety
    ///
    /// If the reserved memory was initialized with data, the caller is responsible for calling
    /// the destructor on the data before releasing the memory.
    pub(crate) unsafe fn release(&mut self, index: usize) {
        let next_free_index = self.next_free_index;

        {
            let entry_meta = self.entry_meta_mut(index);

            if matches!(entry_meta, EntryMeta::Vacant { .. }) {
                panic!(
                    "release({index}) entry was vacant in slab of capacity {}",
                    self.capacity.get()
                );
            }

            *entry_meta = EntryMeta::Vacant { next_free_index };
        }

        // Push the released item's entry onto the free stack.
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
    pub(crate) fn integrity_check(&self) {
        let capacity_value = self.capacity.get();
        let mut observed_is_vacant: Vec<Option<bool>> = vec![None; capacity_value];
        let mut observed_next_free_index: Vec<Option<usize>> = vec![None; capacity_value];
        let mut observed_occupied_count: usize = 0;

        for index in 0..capacity_value {
            match self.entry_meta(index) {
                EntryMeta::Occupied => {
                    observed_is_vacant[index] = Some(false);
                    observed_occupied_count += 1;
                }
                EntryMeta::Vacant { next_free_index } => {
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
            "self.next_free_index points to an occupied slot {} in slab of capacity {}",
            self.next_free_index,
            capacity_value,
        );

        assert!(
            self.count == observed_occupied_count,
            "self.count {} does not match the observed occupied count {} in slab of capacity {}",
            self.count,
            observed_occupied_count,
            capacity_value,
        );

        // Verify that all vacant entries are valid.
        for index in 0..capacity_value {
            if !observed_is_vacant[index].expect("we just populated this above") {
                continue;
            }

            let next_free_index = observed_next_free_index[index]
                .expect("we just populated this above for vacant entries");

            if next_free_index == capacity_value {
                continue;
            }

            assert!(
                next_free_index <= capacity_value,
                "vacant entry {index} has out-of-bounds next_free_index {next_free_index} in slab of capacity {capacity_value}"
            );

            assert!(
                observed_is_vacant[next_free_index].expect("index is in bounds"),
                "vacant entry {index} points to occupied entry {next_free_index} in slab of capacity {capacity_value}"
            );
        }
    }
}

impl Drop for DatalessSlab {
    fn drop(&mut self) {
        let was_empty = self.is_empty();
        let capacity_value = self.capacity.get();

        // Set them all to `Vacant` for form's sake. There is no real data
        // stored in the `EntryMeta` so this does not actually do much.
        for index in 0..capacity_value {
            let entry_meta = self.entry_meta_mut(index);

            *entry_meta = EntryMeta::Vacant {
                next_free_index: capacity_value,
            };
        }

        // SAFETY: The layout matches between alloc and dealloc.
        unsafe {
            dealloc(
                self.first_entry_meta_ptr.as_ptr().cast(),
                self.layout_info.entry_array_layout,
            );
        }

        // We do this check at the end so we clean up the memory first. Mostly to make Miri happy.
        // As we are going to panic anyway if something is wrong, there is little good to expect
        // for the app itself.
        //
        // If we are already panicking, we do not want to panic again because that will
        // simply obscure whatever the original panic was, leading to debug difficulties.
        if !thread::panicking() {
            assert!(
                was_empty,
                "dropped a non-empty DatalessSlab with {} active reservations - this suggests reserved memory may still be in use",
                self.count
            );
        }
    }
}

// SAFETY: There are raw pointers involved here but nothing inherently non-thread-mobile.
// about it, so the slab can move between threads.
unsafe impl Send for DatalessSlab {}

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
    use std::num::NonZero;

    use super::*;

    #[test]
    fn smoke_test() {
        let layout = Layout::new::<u32>();
        let mut slab = DatalessSlab::new(layout, NonZero::new(3).unwrap());

        let reservation_a = slab.reserve();
        let reservation_b = slab.reserve();
        let reservation_c = slab.reserve();

        // Write some values.
        unsafe {
            reservation_a.ptr.cast::<u32>().write(42);
            reservation_b.ptr.cast::<u32>().write(43);
            reservation_c.ptr.cast::<u32>().write(44);
        }

        // Read them back.
        unsafe {
            assert_eq!(reservation_a.ptr.cast::<u32>().read(), 42);
            assert_eq!(reservation_b.ptr.cast::<u32>().read(), 43);
            assert_eq!(reservation_c.ptr.cast::<u32>().read(), 44);
        }

        // Also verify direct ptr usage works.
        unsafe {
            assert_eq!(reservation_a.ptr.cast::<u32>().read(), 42);
            assert_eq!(reservation_b.ptr.cast::<u32>().read(), 43);
            assert_eq!(reservation_c.ptr.cast::<u32>().read(), 44);
        }

        assert_eq!(slab.len(), 3);

        unsafe {
            slab.release(reservation_b.index);
        }

        assert_eq!(slab.len(), 2);

        let reservation_d = slab.reserve();

        unsafe {
            reservation_d.ptr.cast::<u32>().write(45);
            assert_eq!(reservation_a.ptr.cast::<u32>().read(), 42);
            assert_eq!(reservation_c.ptr.cast::<u32>().read(), 44);
            assert_eq!(reservation_d.ptr.cast::<u32>().read(), 45);
        }

        assert!(slab.is_full());

        // Clean up remaining reservations.
        unsafe {
            slab.release(reservation_a.index);
            slab.release(reservation_c.index);
            slab.release(reservation_d.index);
        }
    }
    #[test]
    #[should_panic]
    fn panic_when_full() {
        let layout = Layout::new::<u32>();
        let mut slab = DatalessSlab::new(layout, NonZero::new(3).unwrap());

        _ = slab.reserve();
        _ = slab.reserve();
        _ = slab.reserve();

        _ = slab.reserve();
    }
    #[test]
    #[should_panic]
    fn panic_when_oob_get() {
        let layout = Layout::new::<u32>();
        let mut slab = DatalessSlab::new(layout, NonZero::new(3).unwrap());

        _ = slab.reserve();
        // This test verified that getting an out of bounds index panics.
        // Since we removed get(), we test that creating a pointer to index 1234
        // would panic if we had a way to access arbitrary indices.
        // For now, we'll use entry_meta_ptr as it has bounds checking.
        _ = slab.entry_meta_ptr(1234);
    }
    #[test]
    fn insert_returns_correct_index_and_pointer() {
        let layout = Layout::new::<u32>();
        let mut slab = DatalessSlab::new(layout, NonZero::new(3).unwrap());

        // We expect that we reserve items in order, from the start (0, 1, 2, ...).

        let reservation = slab.reserve();
        assert_eq!(reservation.index(), 0);
        unsafe {
            reservation.ptr().cast::<u32>().write(10);
            assert_eq!(reservation.ptr().cast::<u32>().read(), 10);
        }
        let index_0 = reservation.index();

        let reservation = slab.reserve();
        assert_eq!(reservation.index(), 1);
        unsafe {
            reservation.ptr().cast::<u32>().write(11);
            assert_eq!(reservation.ptr().cast::<u32>().read(), 11);
        }
        let index_1 = reservation.index();

        let reservation = slab.reserve();
        assert_eq!(reservation.index(), 2);
        unsafe {
            reservation.ptr().cast::<u32>().write(12);
            assert_eq!(reservation.ptr().cast::<u32>().read(), 12);
        }
        let index_2 = reservation.index();

        // Clean up reservations before drop.
        unsafe {
            slab.release(index_0);
            slab.release(index_1);
            slab.release(index_2);
        }
    }
    #[test]
    fn release_makes_room() {
        let layout = Layout::new::<u32>();
        let mut slab = DatalessSlab::new(layout, NonZero::new(3).unwrap());

        let reservation_a = slab.reserve();
        let reservation_b = slab.reserve();
        let reservation_c = slab.reserve();

        unsafe {
            reservation_a.ptr.cast::<u32>().write(42);
            reservation_b.ptr.cast::<u32>().write(43);
            reservation_c.ptr.cast::<u32>().write(44);
        }

        unsafe {
            slab.release(reservation_b.index);
        }

        let reservation_d = slab.reserve();
        unsafe {
            reservation_d.ptr.cast::<u32>().write(45);

            assert_eq!(reservation_a.ptr.cast::<u32>().read(), 42);
            assert_eq!(reservation_c.ptr.cast::<u32>().read(), 44);
            assert_eq!(reservation_d.ptr.cast::<u32>().read(), 45);
        }

        // Clean up remaining reservations before drop.
        unsafe {
            slab.release(reservation_a.index);
            slab.release(reservation_c.index);
            slab.release(reservation_d.index);
        }
    }
    #[test]
    #[should_panic]
    fn release_vacant_panics() {
        let layout = Layout::new::<u32>();
        let mut slab = DatalessSlab::new(layout, NonZero::new(3).unwrap());

        unsafe {
            slab.release(1);
        }
    }

    #[test]
    fn different_layouts() {
        // Test with different sized types.
        let layout_u64 = Layout::new::<u64>();
        let mut slab_u64 = DatalessSlab::new(layout_u64, NonZero::new(2).unwrap());
        let reservation = slab_u64.reserve();
        unsafe {
            reservation
                .ptr()
                .cast::<u64>()
                .write(0x1234567890ABCDEF);
            assert_eq!(
                reservation.ptr().cast::<u64>().read(),
                0x1234567890ABCDEF
            );
        }
        unsafe {
            slab_u64.release(reservation.index());
        }

        // Test with larger struct.
        #[repr(C)]
        struct LargeStruct {
            a: u64,
            b: u64,
            c: u64,
            d: u64,
        }

        let layout_large = Layout::new::<LargeStruct>();
        let mut slab_large = DatalessSlab::new(layout_large, NonZero::new(2).unwrap());

        let reservation = slab_large.reserve();
        unsafe {
            reservation
                .ptr()
                .cast::<LargeStruct>()
                .write(LargeStruct {
                    a: 1,
                    b: 2,
                    c: 3,
                    d: 4,
                });
            let value = reservation.ptr().cast::<LargeStruct>().read();
            assert_eq!(value.a, 1);
            assert_eq!(value.b, 2);
            assert_eq!(value.c, 3);
            assert_eq!(value.d, 4);
        }
        unsafe {
            slab_large.release(reservation.index());
        }
    }

    #[test]
    #[should_panic]
    fn zero_capacity_constructor_panics() {
        // This test verifies that NonZero::new(0) panics, maintaining the same behavior
        // as before but now at the type level
        let layout = Layout::new::<usize>();
        drop(DatalessSlab::new(layout, NonZero::new(0).unwrap()));
    }

    #[test]
    #[should_panic]
    fn zero_size_layout_is_panic() {
        let layout = Layout::from_size_align(0, 1).unwrap();
        drop(DatalessSlab::new(layout, NonZero::new(3).unwrap()));
    }

    #[test]
    fn in_refcell_works_fine() {
        use std::cell::RefCell;

        let layout = Layout::new::<u32>();
        let slab = RefCell::new(DatalessSlab::new(layout, NonZero::new(3).unwrap()));

        let (index_a, index_c, index_d);

        {
            let mut slab = slab.borrow_mut();
            let reservation_a = slab.reserve();
            let reservation_b = slab.reserve();
            let reservation_c = slab.reserve();

            index_a = reservation_a.index;
            index_c = reservation_c.index;

            unsafe {
                reservation_a.ptr.cast::<u32>().write(42);
                reservation_b.ptr.cast::<u32>().write(43);
                reservation_c.ptr.cast::<u32>().write(44);

                assert_eq!(reservation_a.ptr.cast::<u32>().read(), 42);
                assert_eq!(reservation_b.ptr.cast::<u32>().read(), 43);
                assert_eq!(reservation_c.ptr.cast::<u32>().read(), 44);
            }

            unsafe {
                slab.release(reservation_b.index);
            }

            let reservation_d = slab.reserve();
            index_d = reservation_d.index;

            unsafe {
                reservation_d.ptr.cast::<u32>().write(45);
                assert_eq!(reservation_a.ptr.cast::<u32>().read(), 42);
                assert_eq!(reservation_c.ptr.cast::<u32>().read(), 44);
                assert_eq!(reservation_d.ptr.cast::<u32>().read(), 45);
            }
        }

        {
            let slab = slab.borrow();
            unsafe {
                assert_eq!(slab.item_ptr(index_a).cast::<u32>().read(), 42);
            }
            assert!(slab.is_full());
        }

        // Clean up remaining reservations before drop.
        {
            let mut slab = slab.borrow_mut();
            unsafe {
                slab.release(index_a);
                slab.release(index_c);
                slab.release(index_d);
            }
        }
    }

    #[test]
    fn multithreaded_via_mutex() {
        use std::sync::{Arc, Mutex};
        use std::thread;

        let layout = Layout::new::<u32>();
        let slab = Arc::new(Mutex::new(DatalessSlab::new(
            layout,
            NonZero::new(3).unwrap(),
        )));

        let a;
        let b;
        let c;

        {
            let mut slab = slab.lock().unwrap();
            let reservation_a = slab.reserve();
            let reservation_b = slab.reserve();
            let reservation_c = slab.reserve();
            a = reservation_a.index;
            b = reservation_b.index;
            c = reservation_c.index;

            unsafe {
                reservation_a.ptr.cast::<u32>().write(42);
                reservation_b.ptr.cast::<u32>().write(43);
                reservation_c.ptr.cast::<u32>().write(44);
            }
        }

        let slab_clone = Arc::clone(&slab);
        let handle = thread::spawn(move || {
            let mut slab = slab_clone.lock().unwrap();

            unsafe {
                slab.release(b);
            }

            let reservation_d = slab.reserve();
            let d = reservation_d.index;

            unsafe {
                reservation_d.ptr.cast::<u32>().write(45);
                assert_eq!(slab.item_ptr(a).cast::<u32>().read(), 42);
                assert_eq!(slab.item_ptr(c).cast::<u32>().read(), 44);
                assert_eq!(reservation_d.ptr.cast::<u32>().read(), 45);
            }

            // Return the index for cleanup.
            d
        });

        let d = handle.join().unwrap();

        {
            let slab = slab.lock().unwrap();
            assert!(slab.is_full());
        }

        // Clean up remaining reservations before drop.
        {
            let mut slab = slab.lock().unwrap();
            unsafe {
                slab.release(a);
                slab.release(c);
                slab.release(d);
            }
        }
    }

    #[test]
    fn insert_returns_correct_pointer() {
        let layout = Layout::new::<u64>();
        let mut slab = DatalessSlab::new(layout, NonZero::new(2).unwrap());

        let reservation = slab.reserve();

        // Verify the pointer works and points to the right location.
        unsafe {
            reservation.ptr().cast::<u64>().write(0xDEADBEEF);
            assert_eq!(reservation.ptr().cast::<u64>().read(), 0xDEADBEEF);
            assert_eq!(reservation.ptr().cast::<u64>().read(), 0xDEADBEEF);
        }

        unsafe {
            slab.release(reservation.index());
        }
    }

    #[test]
    fn stress_test_repeated_reserve_release() {
        let layout = Layout::new::<usize>();
        let mut slab = DatalessSlab::new(layout, NonZero::new(10).unwrap());

        // Fill the slab.
        let mut indices = Vec::new();
        for i in 0..10 {
            let reservation = slab.reserve();
            unsafe {
                reservation.ptr().cast::<usize>().write(i * 100);
            }
            indices.push(reservation.index());
        }

        assert!(slab.is_full());

        // Release every other item.
        for i in (0..10).step_by(2) {
            unsafe {
                slab.release(indices[i]);
            }
        }

        assert_eq!(slab.len(), 5);

        // Fill again.
        for i in (0..10).step_by(2) {
            let reservation = slab.reserve();
            unsafe {
                reservation
                    .ptr()
                    .cast::<usize>()
                    .write(i * 100 + 50);
            }
            indices[i] = reservation.index();
        }

        assert!(slab.is_full());

        // Verify all values are correct.
        for i in 0..10 {
            let expected = if i % 2 == 0 { i * 100 + 50 } else { i * 100 };
            unsafe {
                assert_eq!(
                    slab.item_ptr(indices[i]).cast::<usize>().read(),
                    expected
                );
            }
        }

        // Clean up all reservations before drop.
        for &index in &indices {
            unsafe {
                slab.release(index);
            }
        }
    }

    #[test]
    fn different_alignments() {
        // Test various alignment requirements to ensure proper memory layout.

        // 1-byte aligned.
        #[repr(C, align(1))]
        struct Byte {
            data: u8,
        }

        let mut slab = DatalessSlab::new(Layout::new::<Byte>(), NonZero::new(5).unwrap());
        let reservation = slab.reserve();
        unsafe {
            reservation
                .ptr()
                .cast::<Byte>()
                .write(Byte { data: 42 });
            assert_eq!(reservation.ptr().cast::<Byte>().read().data, 42);
        }
        unsafe {
            slab.release(reservation.index());
        }

        // 2-byte aligned.
        #[repr(C, align(2))]
        struct Word {
            data: u16,
        }

        let mut slab = DatalessSlab::new(Layout::new::<Word>(), NonZero::new(5).unwrap());
        let reservation = slab.reserve();
        unsafe {
            reservation
                .ptr()
                .cast::<Word>()
                .write(Word { data: 0x1234 });
            assert_eq!(
                reservation.ptr().cast::<Word>().read().data,
                0x1234
            );
        }
        unsafe {
            slab.release(reservation.index());
        }

        // 4-byte aligned.
        #[repr(C, align(4))]
        struct DWord {
            data: u32,
        }

        let mut slab = DatalessSlab::new(Layout::new::<DWord>(), NonZero::new(5).unwrap());
        let reservation = slab.reserve();
        unsafe {
            reservation
                .ptr()
                .cast::<DWord>()
                .write(DWord { data: 0x12345678 });
            assert_eq!(
                reservation.ptr().cast::<DWord>().read().data,
                0x12345678
            );
        }
        unsafe {
            slab.release(reservation.index());
        }

        // 8-byte aligned.
        #[repr(C, align(8))]
        struct QWord {
            data: u64,
        }

        let mut slab = DatalessSlab::new(Layout::new::<QWord>(), NonZero::new(5).unwrap());
        let reservation = slab.reserve();
        unsafe {
            reservation.ptr().cast::<QWord>().write(QWord {
                data: 0x123456789ABCDEF0,
            });
            assert_eq!(
                reservation.ptr().cast::<QWord>().read().data,
                0x123456789ABCDEF0
            );
        }
        unsafe {
            slab.release(reservation.index());
        }

        // 16-byte aligned.
        #[repr(C, align(16))]
        struct OWord {
            data: [u64; 2],
        }

        let mut slab = DatalessSlab::new(Layout::new::<OWord>(), NonZero::new(5).unwrap());
        let reservation = slab.reserve();
        unsafe {
            reservation.ptr().cast::<OWord>().write(OWord {
                data: [0x123456789ABCDEF0, 0x0FEDCBA987654321],
            });
            let read_data = reservation.ptr().cast::<OWord>().read();
            assert_eq!(read_data.data[0], 0x123456789ABCDEF0);
            assert_eq!(read_data.data[1], 0x0FEDCBA987654321);
        }
        unsafe {
            slab.release(reservation.index());
        }
    }

    #[test]
    fn complex_data_types() {
        // Test with complex nested structures.
        #[repr(C)]
        struct ComplexStruct {
            a: u8,
            b: u16,
            c: u32,
            d: u64,
            e: [u32; 4],
            f: (u16, u32, u64),
        }

        let mut slab = DatalessSlab::new(Layout::new::<ComplexStruct>(), NonZero::new(3).unwrap());

        let reservation1 = slab.reserve();
        let reservation2 = slab.reserve();

        unsafe {
            // Write complex data to first reservation.
            reservation1
                .ptr()
                .cast::<ComplexStruct>()
                .write(ComplexStruct {
                    a: 0x12,
                    b: 0x3456,
                    c: 0x789ABCDE,
                    d: 0x123456789ABCDEF0,
                    e: [0x11111111, 0x22222222, 0x33333333, 0x44444444],
                    f: (0x5555, 0x66666666, 0x7777777777777777),
                });

            // Write different data to second reservation.
            reservation2
                .ptr()
                .cast::<ComplexStruct>()
                .write(ComplexStruct {
                    a: 0xAB,
                    b: 0xCDEF,
                    c: 0x12345678,
                    d: 0xFEDCBA0987654321,
                    e: [0xAAAAAAAA, 0xBBBBBBBB, 0xCCCCCCCC, 0xDDDDDDDD],
                    f: (0xEEEE, 0xFFFFFFFF, 0x1111111111111111),
                });

            // Verify both can be read back correctly.
            let data1 = reservation1.ptr().cast::<ComplexStruct>().read();
            assert_eq!(data1.a, 0x12);
            assert_eq!(data1.b, 0x3456);
            assert_eq!(data1.c, 0x789ABCDE);
            assert_eq!(data1.d, 0x123456789ABCDEF0);
            assert_eq!(data1.e, [0x11111111, 0x22222222, 0x33333333, 0x44444444]);
            assert_eq!(data1.f, (0x5555, 0x66666666, 0x7777777777777777));

            let data2 = reservation2.ptr().cast::<ComplexStruct>().read();
            assert_eq!(data2.a, 0xAB);
            assert_eq!(data2.b, 0xCDEF);
            assert_eq!(data2.c, 0x12345678);
            assert_eq!(data2.d, 0xFEDCBA0987654321);
            assert_eq!(data2.e, [0xAAAAAAAA, 0xBBBBBBBB, 0xCCCCCCCC, 0xDDDDDDDD]);
            assert_eq!(data2.f, (0xEEEE, 0xFFFFFFFF, 0x1111111111111111));
        }

        unsafe {
            slab.release(reservation1.index());
            slab.release(reservation2.index());
        }
    }

    #[test]
    fn enum_data_types() {
        // Test with enums to ensure proper layout handling.
        #[repr(C)]
        #[allow(dead_code, reason = "test-only enum variants")]
        enum TestEnum {
            Variant1(u32),
            Variant2 { x: u64, y: u64 },
            Variant3,
        }

        let mut slab = DatalessSlab::new(Layout::new::<TestEnum>(), NonZero::new(4).unwrap());

        let res1 = slab.reserve();
        let res2 = slab.reserve();
        let res3 = slab.reserve();

        unsafe {
            // Test different enum variants.
            res1.ptr()
                .cast::<TestEnum>()
                .write(TestEnum::Variant1(0x12345678));
            res2.ptr()
                .cast::<TestEnum>()
                .write(TestEnum::Variant2 {
                    x: 0x1111111111111111,
                    y: 0x2222222222222222,
                });
            res3.ptr()
                .cast::<TestEnum>()
                .write(TestEnum::Variant3);

            // Verify we can read them back (note: this is a bit unsafe since we're
            // treating the enum as raw memory, but it tests the layout handling).
            let _data1 = res1.ptr().cast::<TestEnum>().as_ptr().read();
            let _data2 = res2.ptr().cast::<TestEnum>().as_ptr().read();
            let _data3 = res3.ptr().cast::<TestEnum>().as_ptr().read();
        }

        unsafe {
            slab.release(res1.index());
            slab.release(res2.index());
            slab.release(res3.index());
        }
    }

    #[test]
    fn small_and_large_sizes() {
        // Test very small data types.
        let mut slab_small = DatalessSlab::new(Layout::new::<u8>(), NonZero::new(100).unwrap());
        let mut reservations = Vec::new();

        // Fill with small values.
        for i in 0..100_u8 {
            let reservation = slab_small.reserve();
            unsafe {
                reservation.ptr().cast::<u8>().as_ptr().write(i);
            }
            reservations.push(reservation);
        }

        // Verify all values.
        for (i, reservation) in reservations.iter().enumerate() {
            unsafe {
                assert_eq!(
                    reservation.ptr().cast::<u8>().as_ptr().read(),
                    u8::try_from(i).expect("loop range is within u8 bounds")
                );
            }
        }

        // Clean up.
        for reservation in reservations {
            unsafe {
                slab_small.release(reservation.index());
            }
        }

        // Test large data types.
        #[repr(C)]
        struct HugeStruct {
            data: [u64; 128], // 1024 bytes
        }

        let mut slab_large =
            DatalessSlab::new(Layout::new::<HugeStruct>(), NonZero::new(5).unwrap());
        let reservation = slab_large.reserve();

        unsafe {
            let mut huge_data = HugeStruct { data: [0; 128] };
            for (i, elem) in huge_data.data.iter_mut().enumerate() {
                *elem = (i as u64) * 0x0123456789ABCDEF;
            }

            reservation
                .ptr()
                .cast::<HugeStruct>()
                .as_ptr()
                .write(huge_data);

            let read_data = reservation.ptr().cast::<HugeStruct>().as_ptr().read();
            for (i, &elem) in read_data.data.iter().enumerate() {
                assert_eq!(elem, (i as u64) * 0x0123456789ABCDEF);
            }
        }

        unsafe {
            slab_large.release(reservation.index());
        }
    }

    #[test]
    fn layout_calculation_basic() {
        // Test with a simple u32 layout.
        let item_layout = Layout::new::<u32>();
        let capacity = NonZero::new(5).unwrap();

        let layout_info = SlabLayoutInfo::calculate(item_layout, capacity);

        // The combined entry layout should be larger than just the Entry.
        let entry_layout = Layout::new::<EntryMeta>();
        assert!(layout_info.combined_entry_layout.size() >= entry_layout.size());
        assert!(layout_info.combined_entry_layout.size() >= item_layout.size());

        // Item offset should be non-zero (Entry comes first).
        assert!(layout_info.item_offset > 0);

        // Slab layout should accommodate all entries.
        assert!(
            layout_info.entry_array_layout.size()
                >= layout_info.combined_entry_layout.size() * capacity.get()
        );

        // Alignment should be at least as strict as both Entry and item.
        assert!(layout_info.combined_entry_layout.align() >= entry_layout.align());
        assert!(layout_info.combined_entry_layout.align() >= item_layout.align());
    }

    #[test]
    fn layout_calculation_various_alignments() {
        // Test with different alignment requirements.

        // Test with 1-byte aligned data.
        #[repr(C, align(1))]
        struct Align1 {
            data: u8,
        }

        let layout_info_1 =
            SlabLayoutInfo::calculate(Layout::new::<Align1>(), NonZero::new(3).unwrap());
        assert_eq!(
            layout_info_1.combined_entry_layout.align(),
            Layout::new::<EntryMeta>().align().max(1)
        );

        // Test with 8-byte aligned data.
        #[repr(C, align(8))]
        struct Align8 {
            data: u64,
        }

        let layout_info_8 =
            SlabLayoutInfo::calculate(Layout::new::<Align8>(), NonZero::new(3).unwrap());
        assert_eq!(
            layout_info_8.combined_entry_layout.align(),
            Layout::new::<EntryMeta>().align().max(8)
        );

        // Test with 16-byte aligned data.
        #[repr(C, align(16))]
        struct Align16 {
            data: [u64; 2],
        }

        let layout_info_16 =
            SlabLayoutInfo::calculate(Layout::new::<Align16>(), NonZero::new(3).unwrap());
        assert_eq!(
            layout_info_16.combined_entry_layout.align(),
            Layout::new::<EntryMeta>().align().max(16)
        );
    }

    #[test]
    fn layout_calculation_large_structs() {
        // Test with a large struct to ensure proper layout calculation.
        #[repr(C)]
        struct LargeStruct {
            data: [u64; 32], // 256 bytes
        }

        let item_layout = Layout::new::<LargeStruct>();
        let capacity = NonZero::new(10).unwrap();

        let layout_info = SlabLayoutInfo::calculate(item_layout, capacity);

        // Verify the slab can hold all entries.
        assert!(
            layout_info.entry_array_layout.size()
                >= layout_info.combined_entry_layout.size() * capacity.get()
        );

        // Verify item offset is properly aligned for the large struct.
        assert_eq!(layout_info.item_offset % item_layout.align(), 0);
    }

    #[test]
    #[should_panic(expected = "SlabLayoutInfo cannot be calculated for zero-sized item layout")]
    fn layout_calculation_zero_size_panics() {
        let zero_layout = Layout::from_size_align(0, 1).unwrap();
        #[allow(clippy::let_underscore_must_use, reason = "we expect this to panic")]
        let _ = SlabLayoutInfo::calculate(zero_layout, NonZero::new(3).unwrap());
    }

    #[test]
    fn alignment_stress_test() {
        // This test specifically targets the alignment issue by using a layout
        // that has different alignment requirements than Entry, which forces
        // the combined layout to have padding that could cause misalignment
        // when multiplied by index.

        #[repr(C, align(1))]
        struct ByteAligned {
            data: u8,
        }

        // Create a slab with byte-aligned items, which will create a combined
        // layout where the size might not be a multiple of Entry's alignment
        let mut slab = DatalessSlab::new(Layout::new::<ByteAligned>(), NonZero::new(10).unwrap());

        // Reserve multiple items to stress test the alignment
        let mut reservations = Vec::new();
        for i in 0..10 {
            let reservation = slab.reserve();
            unsafe {
                reservation
                    .ptr()
                    .cast::<ByteAligned>()
                    .as_ptr()
                    .write(ByteAligned {
                        data: u8::try_from(i).expect("loop range is within u8 bounds"),
                    });
            }
            reservations.push(reservation);
        }

        // Verify we can read all items back correctly
        for (i, reservation) in reservations.iter().enumerate() {
            unsafe {
                let value = reservation.ptr().cast::<ByteAligned>().as_ptr().read();
                assert_eq!(
                    value.data,
                    u8::try_from(i).expect("loop range is within u8 bounds")
                );
            }
        }

        // Clean up
        for reservation in reservations {
            unsafe {
                slab.release(reservation.index());
            }
        }
    }

    #[test]
    fn alignment_with_different_item_alignments() {
        // Test alignment with various item alignment requirements to ensure
        // the array layout correctly handles alignment for each entry

        // Test with 1-byte aligned items
        #[repr(C, align(1))]
        struct Align1 {
            data: u8,
        }

        let mut slab1 = DatalessSlab::new(Layout::new::<Align1>(), NonZero::new(5).unwrap());
        let res1 = slab1.reserve();
        unsafe {
            res1.ptr()
                .cast::<Align1>()
                .as_ptr()
                .write(Align1 { data: 42 });
            assert_eq!(res1.ptr().cast::<Align1>().as_ptr().read().data, 42);
        }
        unsafe {
            slab1.release(res1.index());
        }

        // Test with 2-byte aligned items
        #[repr(C, align(2))]
        struct Align2 {
            data: u16,
        }

        let mut slab2 = DatalessSlab::new(Layout::new::<Align2>(), NonZero::new(5).unwrap());
        let res2 = slab2.reserve();
        unsafe {
            res2.ptr()
                .cast::<Align2>()
                .as_ptr()
                .write(Align2 { data: 0x1234 });
            assert_eq!(res2.ptr().cast::<Align2>().as_ptr().read().data, 0x1234);
        }
        unsafe {
            slab2.release(res2.index());
        }

        // Test with multiple entries in each slab to stress alignment
        for _ in 0..3 {
            let res1 = slab1.reserve();
            let res2 = slab2.reserve();
            unsafe {
                res1.ptr()
                    .cast::<Align1>()
                    .as_ptr()
                    .write(Align1 { data: 99 });
                res2.ptr()
                    .cast::<Align2>()
                    .as_ptr()
                    .write(Align2 { data: 0xABCD });
                assert_eq!(res1.ptr().cast::<Align1>().as_ptr().read().data, 99);
                assert_eq!(res2.ptr().cast::<Align2>().as_ptr().read().data, 0xABCD);
            }
            unsafe {
                slab1.release(res1.index());
                slab2.release(res2.index());
            }
        }
    }

    // ...existing tests...
}
