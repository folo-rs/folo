use std::alloc::{Layout, alloc, dealloc};
use std::num::NonZero;
use std::ptr::NonNull;
use std::{mem, ptr, thread};

/// The result of reserving memory in a [`DatalessSlab`].
///
/// Contains both the stable index for later operations and a pointer to the reserved memory.
#[derive(Debug)]
pub(crate) struct SlabReservation {
    /// The stable index that can be used to retrieve or release this memory later.
    index: usize,

    /// A pointer to the reserved memory block.
    ptr: NonNull<()>,
}

impl SlabReservation {
    /// Returns the stable index that can be used to retrieve or release this memory later.
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

/// Provides memory for a specified number of objects with a specific layout without knowing their type.
///
/// A fixed-capacity heap-allocated collection that works with opaque memory blocks. Works similar
/// to a `Vec` but all items are located at stable addresses and the collection has a fixed
/// capacity determined at construction time, operating using an index for lookup. When you reserve memory,
/// you get back the index to use for accessing or deallocating the memory.
///
/// # Out of band access
///
/// The collection does not create or keep references to the memory blocks, so it is valid to access
/// memory via pointers and to create custom references to memory from unsafe code even when not
/// holding an exclusive reference to the collection.
#[derive(Debug)]
pub(crate) struct DatalessSlab {
    /// The maximum number of items this slab can hold.
    capacity: NonZero<usize>,

    /// Layout of one item in the slab. This is only the contents, not including the `Entry`.
    item_layout: Layout,

    /// Offset to add to an `Entry` pointer to get to the actual item inside the entry.
    ///
    /// Essentially, each item in the slab is a combination of `Entry` and the actual item contents,
    /// pseudo-concatenated together in memory (respecting memory layout rules wrt padding).
    item_offset: usize,

    first_entry_ptr: NonNull<Entry>,

    /// Index of the next free slot in the collection. Think of this as a virtual stack of the most
    /// recently freed slots, with the stack entries stored in the collection entries themselves.
    /// Also known as intrusive freelist. This will point out of bounds if the collection is full.
    next_free_index: usize,

    /// The total number of items in the collection. This is not used by the collection itself but
    /// may be valuable to callers who want to know if the collection is empty because in many use
    /// cases the collection is the backing store for a custom reservation/pinning scheme for items
    /// used from unsafe code and may not be valid to drop when any items are still present.
    count: usize,
}

#[derive(Debug)]
enum Entry {
    Occupied,

    Vacant { next_free_index: usize },
}

impl DatalessSlab {
    /// Creates a new slab with the specified item memory layout and capacity.
    ///
    /// # Panics
    ///
    /// Panics if the slab would be zero-sized due to item size being zero.
    #[must_use]
    pub(crate) fn new(item_layout: Layout, capacity: NonZero<usize>) -> Self {
        assert!(
            item_layout.size() > 0,
            "DatalessSlab must have non-zero item size"
        );

        let capacity_value = capacity.get();

        // Calculate the combined layout for Entry + item
        let entry_layout = Layout::new::<Entry>();
        let (combined_layout, item_offset) = entry_layout
            .extend(item_layout)
            .expect("layout extension cannot fail for valid layouts with reasonable sizes");

        // Calculate the layout for the entire slab (array of combined layouts)
        let slab_layout = Layout::from_size_align(
            combined_layout
                .size()
                .checked_mul(capacity_value)
                .expect("capacity multiplication cannot overflow for reasonable capacity values"),
            combined_layout.align(),
        )
        .expect("slab layout calculation cannot fail for valid combined layouts");

        // SAFETY: The layout is valid for the target allocation and not zero-sized
        // (guarded by assertions above).
        let ptr = NonNull::new(unsafe { alloc(slab_layout) })
            .expect("we do not intend to handle allocation failure as a real possibility - OOM results in panic")
            .cast::<Entry>();

        // Initialize all slots to `Vacant` to start with.
        for index in 0_usize..capacity_value {
            let offset = index
                .checked_mul(combined_layout.size())
                .expect("index offset calculation cannot overflow for reasonable index values");

            // SAFETY: We allocated enough space for all items up to the indicated capacity
            // and the offset is calculated safely above.
            let entry_ptr = unsafe { ptr.as_ptr().cast::<u8>().add(offset) };

            // SAFETY: The pointer alignment is guaranteed by the layout calculation.
            #[allow(
                clippy::cast_ptr_alignment,
                reason = "layout calculation ensures proper alignment"
            )]
            let entry_ptr = entry_ptr.cast::<Entry>();

            // SAFETY: The pointer is valid for writes and points to properly allocated memory.
            unsafe {
                ptr::write(
                    entry_ptr,
                    Entry::Vacant {
                        next_free_index: index.checked_add(1_usize).unwrap_or(capacity_value),
                    },
                );
            }
        }

        Self {
            item_layout,
            capacity,
            item_offset,
            first_entry_ptr: ptr,
            next_free_index: 0,
            count: 0,
        }
    }

    /// Layout of the `Entry` and the item it owns.
    #[must_use]
    fn combined_entry_layout(&self) -> Layout {
        let entry_layout = Layout::new::<Entry>();
        entry_layout
            .extend(self.item_layout)
            .expect("layout extension must be valid")
            .0
    }

    /// Layout of the entire slab (all `capacity` entries).
    #[must_use]
    fn slab_layout(&self) -> Layout {
        let combined_layout = self.combined_entry_layout();
        Layout::from_size_align(
            combined_layout
                .size()
                .checked_mul(self.capacity.get())
                .expect("capacity multiplication cannot overflow for reasonable capacity values"),
            combined_layout.align(),
        )
        .expect("slab layout calculation cannot fail for valid combined layouts")
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
            index < self.capacity.get(),
            "entry {index} index out of bounds in slab of capacity {}",
            self.capacity.get()
        );
        let combined_layout = self.combined_entry_layout();

        // Guarded by bounds check above, so we are guaranteed that the pointer is valid.
        // The arithmetic is checked to prevent overflow.
        let offset = index
            .checked_mul(combined_layout.size())
            .expect("offset calculation cannot overflow for reasonable index values");

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
        let entry_ptr = self.entry_ptr(index); // SAFETY: The item_offset is calculated correctly during construction to point to
        // the item contents portion of the entry+item combined layout.
        // SAFETY: entry_ptr is valid and item_offset is calculated correctly
        let data_ptr = unsafe { entry_ptr.as_ptr().cast::<u8>().add(self.item_offset) };

        // SAFETY: The data_ptr is valid and non-null due to the calculations above.
        unsafe { NonNull::new_unchecked(data_ptr.cast::<()>()) }
    }

    /// Returns a pointer to the memory at the specified index.
    ///
    /// # Panics
    ///
    /// Panics if the index is out of bounds or is not associated with reserved memory.
    #[must_use]
    #[cfg(test)]
    pub(crate) fn get(&self, index: usize) -> NonNull<()> {
        match self.entry(index) {
            Entry::Occupied => self.data_ptr(index),
            Entry::Vacant { .. } => panic!(
                "attempted to get unreserved memory at index {index} in slab of capacity {}",
                self.capacity.get()
            ),
        }
    }

    /// Reserves memory in the slab and returns both the index and a pointer to the memory.
    ///
    /// Returns a [`DatalessSlabReservation`] containing the stable index that can be used for later
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
        let mut entry_ptr = self.entry_ptr(index);

        // SAFETY: We are not allowed to perform operations on the slab that would create another
        // reference to the entry (because we hold an exclusive reference). We do not do that, and
        // the slab by design does not create/hold permanent references to its entries.
        let entry = unsafe { entry_ptr.as_mut() };

        let previous_entry = mem::replace(entry, Entry::Occupied);
        self.next_free_index = match previous_entry {
            Entry::Vacant { next_free_index } => next_free_index,
            Entry::Occupied => panic!(
                "entry {index} was not vacant when we reserved it in slab of capacity {}",
                self.capacity.get()
            ),
        };

        let data_ptr = self.data_ptr(index);

        self.count = self
            .count
            .checked_add(1)
            .expect("count cannot overflow because it is bounded by capacity which is bounded by usize::MAX");

        SlabReservation {
            index,
            ptr: data_ptr,
        }
    }

    /// Releases memory that was previously reserved at the given index.
    ///
    /// # Panics
    ///
    /// Panics if the index is out of bounds or is not associated with reserved memory.
    pub(crate) fn release(&mut self, index: usize) {
        let next_free_index = self.next_free_index;

        {
            let entry = self.entry_mut(index);
            if matches!(entry, Entry::Vacant { .. }) {
                panic!(
                    "release({index}) entry was vacant in slab of capacity {}",
                    self.capacity.get()
                );
            }

            *entry = Entry::Vacant { next_free_index };
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

        // Set them all to `Vacant` to ensure any cleanup is done.
        for index in 0..capacity_value {
            let entry = self.entry_mut(index);

            *entry = Entry::Vacant {
                next_free_index: capacity_value,
            };
        }

        // SAFETY: The layout matches between alloc and dealloc.
        unsafe {
            dealloc(self.first_entry_ptr.as_ptr().cast(), self.slab_layout());
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

// SAFETY: There are raw pointers involved here but nothing inherently non-thread-mobile
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

        // Write some values
        unsafe {
            reservation_a.ptr.cast::<u32>().as_ptr().write(42);
            reservation_b.ptr.cast::<u32>().as_ptr().write(43);
            reservation_c.ptr.cast::<u32>().as_ptr().write(44);
        }

        // Read them back
        unsafe {
            assert_eq!(reservation_a.ptr.cast::<u32>().as_ptr().read(), 42);
            assert_eq!(reservation_b.ptr.cast::<u32>().as_ptr().read(), 43);
            assert_eq!(reservation_c.ptr.cast::<u32>().as_ptr().read(), 44);
        }

        // Also verify get() works
        unsafe {
            assert_eq!(
                slab.get(reservation_a.index).cast::<u32>().as_ptr().read(),
                42
            );
            assert_eq!(
                slab.get(reservation_b.index).cast::<u32>().as_ptr().read(),
                43
            );
            assert_eq!(
                slab.get(reservation_c.index).cast::<u32>().as_ptr().read(),
                44
            );
        }

        assert_eq!(slab.len(), 3);

        slab.release(reservation_b.index);

        assert_eq!(slab.len(), 2);

        let reservation_d = slab.reserve();

        unsafe {
            reservation_d.ptr.cast::<u32>().as_ptr().write(45);
            assert_eq!(
                slab.get(reservation_a.index).cast::<u32>().as_ptr().read(),
                42
            );
            assert_eq!(
                slab.get(reservation_c.index).cast::<u32>().as_ptr().read(),
                44
            );
            assert_eq!(
                slab.get(reservation_d.index).cast::<u32>().as_ptr().read(),
                45
            );
        }

        assert!(slab.is_full());

        // Clean up remaining reservations
        slab.release(reservation_a.index);
        slab.release(reservation_c.index);
        slab.release(reservation_d.index);
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
        _ = slab.get(1234);
    }
    #[test]
    fn insert_returns_correct_index_and_pointer() {
        let layout = Layout::new::<u32>();
        let mut slab = DatalessSlab::new(layout, NonZero::new(3).unwrap());

        // We expect that we reserve items in order, from the start (0, 1, 2, ...).

        let reservation = slab.reserve();
        assert_eq!(reservation.index(), 0);
        unsafe {
            reservation.ptr().cast::<u32>().as_ptr().write(10);
            assert_eq!(slab.get(0).cast::<u32>().as_ptr().read(), 10);
        }
        let index_0 = reservation.index();

        let reservation = slab.reserve();
        assert_eq!(reservation.index(), 1);
        unsafe {
            reservation.ptr().cast::<u32>().as_ptr().write(11);
            assert_eq!(slab.get(1).cast::<u32>().as_ptr().read(), 11);
        }
        let index_1 = reservation.index();

        let reservation = slab.reserve();
        assert_eq!(reservation.index(), 2);
        unsafe {
            reservation.ptr().cast::<u32>().as_ptr().write(12);
            assert_eq!(slab.get(2).cast::<u32>().as_ptr().read(), 12);
        }
        let index_2 = reservation.index();

        // Clean up reservations before drop
        slab.release(index_0);
        slab.release(index_1);
        slab.release(index_2);
    }
    #[test]
    fn release_makes_room() {
        let layout = Layout::new::<u32>();
        let mut slab = DatalessSlab::new(layout, NonZero::new(3).unwrap());

        let reservation_a = slab.reserve();
        let reservation_b = slab.reserve();
        let reservation_c = slab.reserve();

        unsafe {
            reservation_a.ptr.cast::<u32>().as_ptr().write(42);
            reservation_b.ptr.cast::<u32>().as_ptr().write(43);
            reservation_c.ptr.cast::<u32>().as_ptr().write(44);
        }

        slab.release(reservation_b.index);

        let reservation_d = slab.reserve();
        unsafe {
            reservation_d.ptr.cast::<u32>().as_ptr().write(45);

            assert_eq!(
                slab.get(reservation_a.index).cast::<u32>().as_ptr().read(),
                42
            );
            assert_eq!(
                slab.get(reservation_c.index).cast::<u32>().as_ptr().read(),
                44
            );
            assert_eq!(
                slab.get(reservation_d.index).cast::<u32>().as_ptr().read(),
                45
            );
        }

        // Clean up remaining reservations before drop
        slab.release(reservation_a.index);
        slab.release(reservation_c.index);
        slab.release(reservation_d.index);
    }
    #[test]
    #[should_panic]
    fn release_vacant_panics() {
        let layout = Layout::new::<u32>();
        let mut slab = DatalessSlab::new(layout, NonZero::new(3).unwrap());

        slab.release(1);
    }

    #[test]
    #[should_panic]
    fn get_vacant_panics() {
        let layout = Layout::new::<u32>();
        let slab = DatalessSlab::new(layout, NonZero::new(3).unwrap());

        _ = slab.get(1);
    }

    #[test]
    fn different_layouts() {
        // Test with different sized types
        let layout_u64 = Layout::new::<u64>();
        let mut slab_u64 = DatalessSlab::new(layout_u64, NonZero::new(2).unwrap());
        let reservation = slab_u64.reserve();
        unsafe {
            reservation
                .ptr()
                .cast::<u64>()
                .as_ptr()
                .write(0x1234567890ABCDEF);
            assert_eq!(
                reservation.ptr().cast::<u64>().as_ptr().read(),
                0x1234567890ABCDEF
            );
        }
        slab_u64.release(reservation.index());

        // Test with larger struct
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
                .as_ptr()
                .write(LargeStruct {
                    a: 1,
                    b: 2,
                    c: 3,
                    d: 4,
                });
            let value = reservation.ptr().cast::<LargeStruct>().as_ptr().read();
            assert_eq!(value.a, 1);
            assert_eq!(value.b, 2);
            assert_eq!(value.c, 3);
            assert_eq!(value.d, 4);
        }
        slab_large.release(reservation.index());
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
                reservation_a.ptr.cast::<u32>().as_ptr().write(42);
                reservation_b.ptr.cast::<u32>().as_ptr().write(43);
                reservation_c.ptr.cast::<u32>().as_ptr().write(44);

                assert_eq!(
                    slab.get(reservation_a.index).cast::<u32>().as_ptr().read(),
                    42
                );
                assert_eq!(
                    slab.get(reservation_b.index).cast::<u32>().as_ptr().read(),
                    43
                );
                assert_eq!(
                    slab.get(reservation_c.index).cast::<u32>().as_ptr().read(),
                    44
                );
            }

            slab.release(reservation_b.index);

            let reservation_d = slab.reserve();
            index_d = reservation_d.index;

            unsafe {
                reservation_d.ptr.cast::<u32>().as_ptr().write(45);
                assert_eq!(
                    slab.get(reservation_a.index).cast::<u32>().as_ptr().read(),
                    42
                );
                assert_eq!(
                    slab.get(reservation_c.index).cast::<u32>().as_ptr().read(),
                    44
                );
                assert_eq!(
                    slab.get(reservation_d.index).cast::<u32>().as_ptr().read(),
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

        // Clean up remaining reservations before drop
        {
            let mut slab = slab.borrow_mut();
            slab.release(index_a);
            slab.release(index_c);
            slab.release(index_d);
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
                reservation_a.ptr.cast::<u32>().as_ptr().write(42);
                reservation_b.ptr.cast::<u32>().as_ptr().write(43);
                reservation_c.ptr.cast::<u32>().as_ptr().write(44);
            }
        }

        let slab_clone = Arc::clone(&slab);
        let handle = thread::spawn(move || {
            let mut slab = slab_clone.lock().unwrap();

            slab.release(b);

            let reservation_d = slab.reserve();
            let d = reservation_d.index;

            unsafe {
                reservation_d.ptr.cast::<u32>().as_ptr().write(45);
                assert_eq!(slab.get(a).cast::<u32>().as_ptr().read(), 42);
                assert_eq!(slab.get(c).cast::<u32>().as_ptr().read(), 44);
                assert_eq!(
                    slab.get(reservation_d.index).cast::<u32>().as_ptr().read(),
                    45
                );
            }

            // Return the index for cleanup
            d
        });

        let d = handle.join().unwrap();

        {
            let slab = slab.lock().unwrap();
            assert!(slab.is_full());
        }

        // Clean up remaining reservations before drop
        {
            let mut slab = slab.lock().unwrap();
            slab.release(a);
            slab.release(c);
            slab.release(d);
        }
    }

    #[test]
    fn insert_returns_correct_pointer() {
        let layout = Layout::new::<u64>();
        let mut slab = DatalessSlab::new(layout, NonZero::new(2).unwrap());

        let reservation = slab.reserve();

        // Verify the pointer works and points to the right location
        unsafe {
            reservation.ptr().cast::<u64>().as_ptr().write(0xDEADBEEF);
            assert_eq!(
                slab.get(reservation.index()).cast::<u64>().as_ptr().read(),
                0xDEADBEEF
            );
            assert_eq!(reservation.ptr().cast::<u64>().as_ptr().read(), 0xDEADBEEF);
        }

        slab.release(reservation.index());
    }

    #[test]
    fn stress_test_repeated_reserve_release() {
        let layout = Layout::new::<usize>();
        let mut slab = DatalessSlab::new(layout, NonZero::new(10).unwrap());

        // Fill the slab
        let mut indices = Vec::new();
        for i in 0..10 {
            let reservation = slab.reserve();
            unsafe {
                reservation.ptr().cast::<usize>().as_ptr().write(i * 100);
            }
            indices.push(reservation.index());
        }

        assert!(slab.is_full());

        // Release every other item
        for i in (0..10).step_by(2) {
            slab.release(indices[i]);
        }

        assert_eq!(slab.len(), 5);

        // Fill again
        for i in (0..10).step_by(2) {
            let reservation = slab.reserve();
            unsafe {
                reservation
                    .ptr()
                    .cast::<usize>()
                    .as_ptr()
                    .write(i * 100 + 50);
            }
            indices[i] = reservation.index();
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

        // Clean up all reservations before drop
        for &index in &indices {
            slab.release(index);
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
        let mut slab = DatalessSlab::new(layout, NonZero::new(2).unwrap());

        let reservation = slab.reserve();

        // Verify alignment
        assert_eq!(reservation.ptr().as_ptr() as usize % 16, 0);

        unsafe {
            let aligned_ptr = reservation.ptr().cast::<AlignedStruct>();
            aligned_ptr
                .as_ptr()
                .write(AlignedStruct { data: [0x42; 32] });
            let value = aligned_ptr.as_ptr().read();
            assert_eq!(value.data[0], 0x42);
            assert_eq!(value.data[31], 0x42);
        }

        slab.release(reservation.index());
    }
    #[test]
    fn drop_with_no_active_reservations_does_not_panic() {
        let layout = Layout::new::<u64>();
        let capacity = NonZero::new(10).unwrap();
        let mut slab = DatalessSlab::new(layout, capacity);

        // Reserve and then release immediately
        let reservation = slab.reserve();
        slab.release(reservation.index());

        assert!(slab.is_empty());

        // Slab should drop without panic
        drop(slab);
    }

    #[test]
    #[should_panic(expected = "dropped a non-empty DatalessSlab with 1 active reservations")]
    fn drop_with_active_reservation_panics() {
        let layout = Layout::new::<u64>();
        let capacity = NonZero::new(10).unwrap();
        let mut slab = DatalessSlab::new(layout, capacity);

        let _reservation = slab.reserve();

        // Slab should panic on drop since we still have an active reservation
        drop(slab);
    }

    #[test]
    #[should_panic(expected = "dropped a non-empty DatalessSlab with 3 active reservations")]
    fn drop_with_multiple_active_reservations_panics() {
        let layout = Layout::new::<u32>();
        let capacity = NonZero::new(10).unwrap();
        let mut slab = DatalessSlab::new(layout, capacity);

        let _reservation1 = slab.reserve();
        let _reservation2 = slab.reserve();
        let _reservation3 = slab.reserve();

        // Slab should panic on drop since we have multiple active reservations
        drop(slab);
    }

    #[test]
    #[should_panic(expected = "dropped a non-empty DatalessSlab with 2 active reservations")]
    fn drop_with_some_released_reservations_still_panics() {
        let layout = Layout::new::<i64>();
        let capacity = NonZero::new(10).unwrap();
        let mut slab = DatalessSlab::new(layout, capacity);

        let reservation1 = slab.reserve();
        let _reservation2 = slab.reserve();
        let _reservation3 = slab.reserve();

        // Release one reservation but keep two
        slab.release(reservation1.index());

        // Slab should still panic since we have active reservations
        drop(slab);
    }
}
