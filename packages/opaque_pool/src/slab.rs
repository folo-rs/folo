use std::alloc::{Layout, alloc, dealloc};
use std::mem::MaybeUninit;
use std::num::NonZero;
use std::ptr::NonNull;
use std::{mem, ptr, thread};

use crate::{DropPolicy, Dropper};

/// Stores a specified number of objects with a specific layout without
/// remembering their type, allowing objects of mixed types in the same slab.
///
/// # Out of band access
///
/// The collection does not create or keep references to the memory blocks, so it is valid to access
/// memory via pointers and to create custom references to memory from unsafe code even when not
/// holding an exclusive reference to the collection.
#[derive(Debug)]
pub(crate) struct OpaqueSlab {
    /// Maximum number of items that can be stored, ensuring capacity bounds checking
    /// during insertion operations.
    capacity: NonZero<usize>,

    /// Memory layout requirements for items stored in this slab, used for debug assertions
    /// to validate type safety when inserting values.
    #[cfg_attr(
        not(debug_assertions),
        expect(
            dead_code,
            reason = "Used in cfg(debug_assertions) for type layout checking"
        )
    )]
    item_layout: Layout,

    /// Precomputed layout calculations for efficient memory access patterns and allocation.
    layout_info: SlabLayoutInfo,

    /// Base pointer for the contiguous memory block containing all slab entries, where each
    /// entry combines metadata and item storage with proper alignment padding.
    first_entry_meta_ptr: NonNull<EntryMeta>,

    /// Head of the intrusive freelist implementing a stack of available slots, where each
    /// vacant entry stores the index of the next free slot. Points beyond capacity when full.
    next_free_index: usize,

    /// Current number of occupied slots, enabling detection of memory leaks when the slab
    /// is dropped while items are still allocated.
    count: usize,

    /// Drop policy that determines whether the slab panics if items are present during drop.
    drop_policy: DropPolicy,
}

/// The result of inserting a value into a [`OpaqueSlab`].
#[derive(Debug)]
pub(crate) struct SlabItem<T> {
    /// Slab index where this item is stored, required for removal operations.
    index: usize,

    /// Direct pointer to the stored value, enabling efficient access without
    /// additional indirection through the slab.
    ptr: NonNull<T>,
}

impl<T> SlabItem<T> {
    /// Returns the index where this item is stored in the slab.
    ///
    /// This index can be used to remove the item from the slab later.
    #[must_use]
    pub(crate) fn index(&self) -> usize {
        self.index
    }

    /// Returns a pointer to the inserted value.
    #[must_use]
    pub(crate) fn ptr(&self) -> NonNull<T> {
        self.ptr
    }
}

/// Layout calculations for a [`OpaqueSlab`].
#[derive(Clone, Debug, Eq, PartialEq)]
struct SlabLayoutInfo {
    /// Combined memory layout ensuring proper alignment between metadata and item storage,
    /// with the size representing the stride for array element access.
    combined_entry_layout: Layout,

    /// Byte offset from entry metadata to item data, calculated to maintain proper
    /// alignment requirements for the stored item type.
    item_offset: usize,

    /// Total memory layout for the entire slab allocation, encompassing all entries
    /// for the specified capacity with proper alignment and padding.
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

/// Metadata for each entry in the slab, tracking occupancy state and enabling
/// type-erased value management through droppers.
#[derive(Debug)]
enum EntryMeta {
    /// Entry contains a valid item with associated dropper for proper cleanup.
    /// The actual item data follows this metadata at the calculated offset.
    ///
    /// When this variant is dropped (via assignment, `mem::replace`, or going out of scope),
    /// the dropper field is automatically dropped, which in turn causes the item to be dropped.
    Occupied {
        /// Type-erased dropper that properly destroys the stored item when dropped.
        _dropper: Dropper,
    },

    /// Entry is available for allocation, forming part of the intrusive freelist.
    Vacant {
        /// Index of the next available slot in the freelist chain.
        next_free_index: usize,
    },
}

impl OpaqueSlab {
    /// Creates a new slab with the specified item memory layout, capacity, and drop policy.
    ///
    /// # Panics
    ///
    /// Panics if the slab would be zero-sized due to item size being zero.
    #[must_use]
    pub(crate) fn new(
        item_layout: Layout,
        capacity: NonZero<usize>,
        drop_policy: DropPolicy,
    ) -> Self {
        let layout_info = SlabLayoutInfo::calculate(item_layout, capacity);

        // SAFETY: The layout_info.entry_array_layout is guaranteed to be valid and non-zero-sized
        // by SlabLayoutInfo::calculate, which validates the item_layout.size() > 0 and performs
        // all layout calculations without overflow.
        let first_entry_ptr = NonNull::new(unsafe { alloc(layout_info.entry_array_layout) })
            .expect("we do not intend to handle allocation failure as a real possibility - OOM results in panic")
            .cast::<EntryMeta>();

        // Initialize all slots to `Vacant` to start with.
        for index in 0_usize..capacity.get() {
            // Cannot overflow because that would imply our capacity extends beyond virtual memory.
            let offset = index.wrapping_mul(layout_info.combined_entry_layout.size());

            // SAFETY: We allocated memory for capacity.get() entries above, and index is bounded
            // by the loop condition (index < capacity.get()), ensuring we stay within bounds.
            let entry_meta_ptr = unsafe { first_entry_ptr.as_ptr().cast::<u8>().add(offset) };

            #[expect(
                clippy::cast_ptr_alignment,
                reason = "Layout::pad_to_align() ensures proper alignment between array elements"
            )]
            let entry_meta_ptr = entry_meta_ptr.cast::<EntryMeta>();

            // SAFETY: entry_meta_ptr points to valid, properly allocated and aligned memory within
            // our allocated block, as guaranteed by the bounds check and alignment calculations above.
            unsafe {
                ptr::write(
                    entry_meta_ptr,
                    EntryMeta::Vacant {
                        // Cannot overflow, as that would imply the slab
                        // is longer than virtual memory.
                        next_free_index: index.wrapping_add(1),
                    },
                );
            }
        }

        Self {
            capacity,
            item_layout,
            layout_info,
            first_entry_meta_ptr: first_entry_ptr,
            next_free_index: 0,
            count: 0,
            drop_policy,
        }
    }

    /// Returns the number of reserved memory blocks in the slab.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Can be mutated to infinitely growing memory use and/or infinite loop.
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

    #[expect(clippy::needless_pass_by_ref_mut, reason = "false positive")]
    fn entry_meta_mut(&mut self, index: usize) -> &mut EntryMeta {
        let mut entry_meta_ptr = self.entry_meta_ptr(index);

        // SAFETY: entry_meta_ptr was validated by entry_meta_ptr() bounds checking and points to
        // an initialized EntryMeta that we own exclusively (we hold &mut self).
        unsafe { entry_meta_ptr.as_mut() }
    }

    fn entry_meta_ptr(&self, index: usize) -> NonNull<EntryMeta> {
        assert!(
            index < self.capacity.get(),
            "entry {index} index out of bounds in slab of capacity {}",
            self.capacity.get()
        );

        // Guarded by bounds check above, so we are guaranteed that the pointer is valid.
        // This cannot overflow because that would imply the slab extends beyond virtual memory.
        let offset = index.wrapping_mul(self.layout_info.combined_entry_layout.size());

        // SAFETY: first_entry_meta_ptr is valid from our allocation in new(), offset is within
        // bounds due to the index bounds check above, and byte_add preserves pointer validity.
        unsafe { self.first_entry_meta_ptr.byte_add(offset) }
    }

    fn item_ptr<T>(&self, index: usize) -> NonNull<T> {
        let entry_meta_ptr = self.entry_meta_ptr(index);

        // SAFETY: entry_meta_ptr is valid from entry_meta_ptr() and item_offset was calculated
        // correctly by SlabLayoutInfo::calculate() to point to the item portion of the entry.
        unsafe {
            entry_meta_ptr
                .byte_add(self.layout_info.item_offset)
                .cast::<T>()
        }
    }

    /// Inserts a value into the slab and returns an object that provides both the item's index
    /// and a pointer to the pinned item.
    ///
    /// # Panics
    ///
    /// Panics if the slab is full.
    ///
    /// In debug builds, panics if the layout of `T` does not match the slab's item layout.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the layout of `T` matches the slab's item layout.
    /// In debug builds, this is checked with an assertion.
    #[cfg(test)]
    #[must_use]
    pub(crate) unsafe fn insert<T>(&mut self, value: T) -> SlabItem<T> {
        // Implement insert() in terms of insert_with() to reduce logic duplication.
        // SAFETY: Forwarding safety requirements to the caller.
        unsafe {
            self.insert_with(|uninit: &mut MaybeUninit<T>| {
                uninit.write(value);
            })
        }
    }

    /// Inserts a value into the slab using in-place initialization and returns a handle to it.
    ///
    /// This allows the caller to initialize the item in-place using a closure that receives
    /// a `&mut MaybeUninit<T>`. This can be more efficient than constructing the value
    /// separately and then moving it into the slab, especially for large or complex types.
    ///
    /// # Panics
    ///
    /// Panics if the slab is full or if the layout of `T` does not match the slab's item layout
    /// (in debug builds only).
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    /// - The layout of `T` matches the slab's item layout.
    /// - The closure properly initializes the `MaybeUninit<T>` before returning.
    ///
    /// In debug builds, the layout requirement is checked with an assertion.
    #[must_use]
    pub(crate) unsafe fn insert_with<T>(
        &mut self,
        f: impl FnOnce(&mut MaybeUninit<T>),
    ) -> SlabItem<T> {
        #[cfg(debug_assertions)]
        {
            assert_eq!(
                Layout::new::<T>(),
                self.item_layout,
                "type layout mismatch: expected layout {:?}, got layout {:?}",
                self.item_layout,
                Layout::new::<T>()
            );
        }

        assert!(
            !self.is_full(),
            "cannot insert value into a full slab of capacity {}",
            self.capacity.get()
        );

        // Pop the next free index from the stack of free entries.
        let index = self.next_free_index;
        let mut entry_meta_ptr = self.entry_meta_ptr(index);

        // SAFETY: We hold an exclusive reference to the slab (&mut self), and entry_meta_ptr
        // points to a valid, initialized EntryMeta. The slab design ensures no other references
        // to individual entries exist while we hold the exclusive slab reference.
        let entry_meta_ptr = unsafe { entry_meta_ptr.as_mut() };

        // Get the item pointer where we'll write the value.
        let item_ptr = self.item_ptr::<T>(index);

        // Create a MaybeUninit wrapper around the memory location and call the initialization function.
        // SAFETY: item_ptr points to valid, properly aligned memory for type T within our allocated
        // block, and we own this memory exclusively. The caller guarantees proper initialization.
        unsafe {
            let mut uninit_ptr = item_ptr.cast::<MaybeUninit<T>>();
            f(uninit_ptr.as_mut());
        }

        // Create a dropper for the value we just initialized.
        // SAFETY: pointer is valid and properly initialized by the closure, and we ensure the dropper
        // will be called before the memory is deallocated or reused.
        let dropper = unsafe { Dropper::new(item_ptr) };

        // Update the entry metadata to mark it as occupied and store the dropper.
        let previous_entry =
            mem::replace(entry_meta_ptr, EntryMeta::Occupied { _dropper: dropper });

        self.next_free_index = match previous_entry {
            EntryMeta::Vacant { next_free_index } => next_free_index,
            EntryMeta::Occupied { .. } => {
                panic!(
                    "insert_with({index}) entry was already occupied in slab of capacity {}",
                    self.capacity.get()
                );
            }
        };

        // Increment the count since we successfully inserted an item.
        // Cannot overflow because we would hit capacity limits or virtual memory limits first.
        self.count = self.count.wrapping_add(1);

        #[cfg(debug_assertions)]
        self.integrity_check();

        SlabItem {
            index,
            ptr: item_ptr,
        }
    }

    /// Removes a value from the slab that was previously inserted at the given index.
    ///
    /// The value is properly dropped using its stored dropper before the memory is freed.
    ///
    /// # Panics
    ///
    /// Panics if the index is out of bounds or is not associated with an inserted value.
    pub(crate) fn remove(&mut self, index: usize) {
        let next_free_index = self.next_free_index;

        {
            let entry_meta = self.entry_meta_mut(index);

            // Ensure the entry was occupied and replace it with Vacant.
            // The Drop implementation of EntryMeta will automatically call the dropper.
            let was_occupied = match mem::replace(entry_meta, EntryMeta::Vacant { next_free_index })
            {
                EntryMeta::Vacant { .. } => false,
                EntryMeta::Occupied { .. } => true,
            };

            assert!(
                was_occupied,
                "remove({index}) entry was vacant in slab of capacity {}",
                self.capacity.get()
            );
        }

        // Push the released item's entry onto the free stack.
        self.next_free_index = index;

        // Cannot overflow because we asserted above the removed entry was occupied.
        self.count = self.count.wrapping_sub(1);
    }

    /// Removes a value from the slab and returns it.
    ///
    /// This method moves the value out of the slab and returns ownership to the caller.
    /// The memory slot is marked as vacant and becomes available for future insertions.
    ///
    /// # Panics
    ///
    /// Panics if the index is out of bounds or is not associated with an inserted value.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the value at `index` is actually of type `T`.
    pub(crate) unsafe fn remove_unpin<T: Unpin>(&mut self, index: usize) -> T {
        #[cfg(debug_assertions)]
        {
            assert_eq!(
                Layout::new::<T>(),
                self.item_layout,
                "type layout mismatch: expected layout {:?}, got layout {:?}",
                self.item_layout,
                Layout::new::<T>()
            );
        }

        let next_free_index = self.next_free_index;
        let item_ptr = self.item_ptr::<T>(index);

        {
            let entry_meta = self.entry_meta_mut(index);

            // Ensure the entry was occupied and replace it with Vacant.
            // We deliberately ignore the dropper to avoid dropping the value.
            let was_occupied = match mem::replace(entry_meta, EntryMeta::Vacant { next_free_index })
            {
                EntryMeta::Vacant { .. } => false,
                EntryMeta::Occupied { _dropper } => {
                    // Prevent the dropper from running by forgetting it.
                    mem::forget(_dropper);
                    true
                }
            };

            assert!(
                was_occupied,
                "remove_unpin({index}) entry was vacant in slab of capacity {}",
                self.capacity.get()
            );
        }

        // Read the value from memory before updating the slab state.
        // SAFETY: The pointer is valid and contains an initialized value of type T.
        // We have exclusive access through &mut self, and we've verified the entry was occupied.
        let value = unsafe { item_ptr.read() };

        // Push the released item's entry onto the free stack.
        self.next_free_index = index;

        // Cannot overflow because we asserted above the removed entry was occupied.
        self.count = self.count.wrapping_sub(1);

        value
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
            let entry_meta_ptr = self.entry_meta_ptr(index);

            // SAFETY: entry_meta_ptr was validated by entry_meta_ptr() bounds checking and points to
            // an initialized EntryMeta that we own exclusively (we hold &self).
            let entry_meta = unsafe { entry_meta_ptr.as_ref() };

            match entry_meta {
                EntryMeta::Occupied { .. } => {
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

impl Drop for OpaqueSlab {
    fn drop(&mut self) {
        let was_empty = self.is_empty();
        let original_count = self.count;
        let capacity_value = self.capacity.get();

        // Manually drop all EntryMeta instances to ensure occupied entries are properly dropped.
        // This will automatically call the dropper for any Occupied entries.
        for index in 0..capacity_value {
            let entry_meta_ptr = self.entry_meta_ptr(index);

            // SAFETY: We allocated and initialized these EntryMeta instances in new(),
            // and we're dropping them exactly once before deallocating the memory.
            unsafe {
                ptr::drop_in_place(entry_meta_ptr.as_ptr());
            }
        }

        // SAFETY: We allocated memory using layout_info.entry_array_layout in new(), and we're
        // deallocating with the same layout. The memory was not yet deallocated and the layout
        // parameters remain valid.
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
        if !thread::panicking() && matches!(self.drop_policy, DropPolicy::MustNotDropItems) {
            assert!(
                was_empty,
                "dropped a non-empty OpaqueSlab with {original_count} items - this is forbidden by DropPolicy::MustNotDropItems"
            );
        }
    }
}

// SAFETY: OpaqueSlab contains raw pointers (NonNull<EntryMeta>) but they are used purely
// for memory management within the slab's owned allocation. The slab does not share these
// pointers with other threads and does not rely on thread-local state. All pointer operations
// are protected by Rust's borrowing rules through the &self/&mut self methods.
unsafe impl Send for OpaqueSlab {}

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
    use crate::DropPolicy;

    #[test]
    fn smoke_test() {
        let layout = Layout::new::<u32>();
        let mut slab = OpaqueSlab::new(layout, NonZero::new(3).unwrap(), DropPolicy::MayDropItems);

        let pooled_a = unsafe { slab.insert(42_u32) };
        let pooled_b = unsafe { slab.insert(43_u32) };
        let pooled_c = unsafe { slab.insert(44_u32) };

        // Read them back using the typed interface.
        unsafe {
            assert_eq!(pooled_a.ptr().read(), 42);
            assert_eq!(pooled_b.ptr().read(), 43);
            assert_eq!(pooled_c.ptr().read(), 44);
        }

        // Also verify direct reference access works.
        unsafe {
            assert_eq!(*pooled_a.ptr().as_ref(), 42);
            assert_eq!(*pooled_b.ptr().as_ref(), 43);
            assert_eq!(*pooled_c.ptr().as_ref(), 44);
        }

        // Test mutable reference access as well.
        unsafe {
            *pooled_a.ptr().as_mut() += 1;
            *pooled_b.ptr().as_mut() += 1;
            *pooled_c.ptr().as_mut() += 1;
            assert_eq!(*pooled_a.ptr().as_ref(), 43);
            assert_eq!(*pooled_b.ptr().as_ref(), 44);
            assert_eq!(*pooled_c.ptr().as_ref(), 45);
        }

        assert_eq!(slab.len(), 3);

        slab.remove(pooled_b.index);

        assert_eq!(slab.len(), 2);

        let pooled_d = unsafe { slab.insert(45_u32) };

        unsafe {
            assert_eq!(pooled_a.ptr().read(), 43); // Was incremented by 1 in earlier test
            assert_eq!(pooled_c.ptr().read(), 45); // Was incremented by 1 in earlier test  
            assert_eq!(pooled_d.ptr().read(), 45);
        }

        assert!(slab.is_full());

        // Clean up remaining items, except for C which we allow the slab to clean up.
        slab.remove(pooled_a.index);
        slab.remove(pooled_d.index);
    }

    #[test]
    #[should_panic]
    fn panic_when_full() {
        let layout = Layout::new::<u32>();
        let mut slab = OpaqueSlab::new(layout, NonZero::new(3).unwrap(), DropPolicy::MayDropItems);

        _ = unsafe { slab.insert(1_u32) };
        _ = unsafe { slab.insert(2_u32) };
        _ = unsafe { slab.insert(3_u32) };

        _ = unsafe { slab.insert(4_u32) };
    }

    #[test]
    fn insert_returns_correct_index_and_pointer() {
        let layout = Layout::new::<u32>();
        let mut slab = OpaqueSlab::new(layout, NonZero::new(3).unwrap(), DropPolicy::MayDropItems);

        // We expect that we insert items in order, from the start (0, 1, 2, ...).

        let pooled = unsafe { slab.insert(10_u32) };
        assert_eq!(pooled.index(), 0);
        unsafe {
            assert_eq!(pooled.ptr().read(), 10);
        }

        let pooled = unsafe { slab.insert(11_u32) };
        assert_eq!(pooled.index(), 1);
        unsafe {
            assert_eq!(pooled.ptr().read(), 11);
        }

        let pooled = unsafe { slab.insert(12_u32) };
        assert_eq!(pooled.index(), 2);
        unsafe {
            assert_eq!(pooled.ptr().read(), 12);
        }
    }

    #[test]
    fn remove_makes_room() {
        let layout = Layout::new::<u32>();
        let mut slab = OpaqueSlab::new(layout, NonZero::new(3).unwrap(), DropPolicy::MayDropItems);

        let pooled_a = unsafe { slab.insert(42_u32) };
        let pooled_b = unsafe { slab.insert(43_u32) };
        let pooled_c = unsafe { slab.insert(44_u32) };

        slab.remove(pooled_b.index);

        let pooled_d = unsafe { slab.insert(45_u32) };

        unsafe {
            assert_eq!(pooled_a.ptr().read(), 42);
            assert_eq!(pooled_c.ptr().read(), 44);
            assert_eq!(pooled_d.ptr().read(), 45);
        }
    }

    #[test]
    #[should_panic]
    fn remove_vacant_panics() {
        let layout = Layout::new::<u32>();
        let mut slab = OpaqueSlab::new(layout, NonZero::new(3).unwrap(), DropPolicy::MayDropItems);

        slab.remove(1);
    }

    #[test]
    #[should_panic]
    fn zero_capacity_constructor_panics() {
        let layout = Layout::new::<usize>();

        drop(OpaqueSlab::new(
            layout,
            NonZero::new(0).unwrap(),
            DropPolicy::MayDropItems,
        ));
    }

    #[test]
    #[should_panic]
    fn zero_size_layout_is_panic() {
        let layout = Layout::from_size_align(0, 1).unwrap();

        drop(OpaqueSlab::new(
            layout,
            NonZero::new(3).unwrap(),
            DropPolicy::MayDropItems,
        ));
    }

    #[test]
    fn in_refcell_works_fine() {
        use std::cell::RefCell;

        let layout = Layout::new::<u32>();
        let slab = RefCell::new(OpaqueSlab::new(
            layout,
            NonZero::new(3).unwrap(),
            DropPolicy::MayDropItems,
        ));

        {
            let mut slab = slab.borrow_mut();
            let item_a = unsafe { slab.insert(42_u32) };
            let item_b = unsafe { slab.insert(43_u32) };
            let item_c = unsafe { slab.insert(44_u32) };

            unsafe {
                assert_eq!(item_a.ptr().read(), 42);
                assert_eq!(item_b.ptr().read(), 43);
                assert_eq!(item_c.ptr().read(), 44);
            }

            slab.remove(item_b.index());

            let item_d = unsafe { slab.insert(45_u32) };

            unsafe {
                assert_eq!(item_a.ptr().read(), 42);
                assert_eq!(item_c.ptr().read(), 44);
                assert_eq!(item_d.ptr().read(), 45);
            }
        }

        {
            let slab = slab.borrow();
            assert!(slab.is_full());
        }
    }

    #[test]
    fn multithreaded_via_mutex() {
        use std::sync::{Arc, Mutex};
        use std::thread;

        let layout = Layout::new::<u32>();
        let slab = Arc::new(Mutex::new(OpaqueSlab::new(
            layout,
            NonZero::new(3).unwrap(),
            DropPolicy::MayDropItems,
        )));

        let b;

        {
            let mut slab = slab.lock().unwrap();
            // SAFETY: The layout of u32 matches the slab's layout.
            let item_a = unsafe { slab.insert(42_u32) };
            // SAFETY: The layout of u32 matches the slab's layout.
            let item_b = unsafe { slab.insert(43_u32) };
            // SAFETY: The layout of u32 matches the slab's layout.
            let item_c = unsafe { slab.insert(44_u32) };
            b = item_b.index();

            unsafe {
                assert_eq!(item_a.ptr().read(), 42);
                assert_eq!(item_b.ptr().read(), 43);
                assert_eq!(item_c.ptr().read(), 44);
            }
        }

        let slab_clone = Arc::clone(&slab);
        let handle = thread::spawn(move || {
            let mut slab = slab_clone.lock().unwrap();

            slab.remove(b);

            // SAFETY: The layout of u32 matches the slab's layout.
            let item_d = unsafe { slab.insert(45_u32) };

            unsafe {
                assert_eq!(item_d.ptr().read(), 45);
            }
        });

        handle.join().unwrap();

        {
            let slab = slab.lock().unwrap();
            assert!(slab.is_full());
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

        let mut slab = OpaqueSlab::new(
            Layout::new::<Byte>(),
            NonZero::new(5).unwrap(),
            DropPolicy::MayDropItems,
        );
        // SAFETY: The layout of Byte matches the slab's layout.
        let item = unsafe { slab.insert(Byte { data: 42 }) };
        unsafe {
            assert_eq!(item.ptr().read().data, 42);
        }
        slab.remove(item.index());

        // 2-byte aligned.
        #[repr(C, align(2))]
        struct Word {
            data: u16,
        }

        let mut slab = OpaqueSlab::new(
            Layout::new::<Word>(),
            NonZero::new(5).unwrap(),
            DropPolicy::MayDropItems,
        );
        // SAFETY: The layout of Word matches the slab's layout.
        let item = unsafe { slab.insert(Word { data: 0x1234 }) };
        unsafe {
            assert_eq!(item.ptr().read().data, 0x1234);
        }
        slab.remove(item.index());

        // 4-byte aligned.
        #[repr(C, align(4))]
        struct DWord {
            data: u32,
        }

        let mut slab = OpaqueSlab::new(
            Layout::new::<DWord>(),
            NonZero::new(5).unwrap(),
            DropPolicy::MayDropItems,
        );
        // SAFETY: The layout of DWord matches the slab's layout.
        let item = unsafe { slab.insert(DWord { data: 0x12345678 }) };
        unsafe {
            assert_eq!(item.ptr().read().data, 0x12345678);
        }
        slab.remove(item.index());

        // 8-byte aligned.
        #[repr(C, align(8))]
        struct QWord {
            data: u64,
        }

        let mut slab = OpaqueSlab::new(
            Layout::new::<QWord>(),
            NonZero::new(5).unwrap(),
            DropPolicy::MayDropItems,
        );
        // SAFETY: The layout of QWord matches the slab's layout.
        let item = unsafe {
            slab.insert(QWord {
                data: 0x123456789ABCDEF0,
            })
        };
        unsafe {
            assert_eq!(item.ptr().read().data, 0x123456789ABCDEF0);
        }
        slab.remove(item.index());

        // 16-byte aligned.
        #[repr(C, align(16))]
        struct OWord {
            data: [u64; 2],
        }

        let mut slab = OpaqueSlab::new(
            Layout::new::<OWord>(),
            NonZero::new(5).unwrap(),
            DropPolicy::MayDropItems,
        );
        // SAFETY: The layout of OWord matches the slab's layout.
        let item = unsafe {
            slab.insert(OWord {
                data: [0x123456789ABCDEF0, 0x0FEDCBA987654321],
            })
        };
        unsafe {
            let read_data = item.ptr().read();
            assert_eq!(read_data.data[0], 0x123456789ABCDEF0);
            assert_eq!(read_data.data[1], 0x0FEDCBA987654321);
        }
        slab.remove(item.index());
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

        let mut slab = OpaqueSlab::new(
            Layout::new::<ComplexStruct>(),
            NonZero::new(3).unwrap(),
            DropPolicy::MayDropItems,
        );

        // SAFETY: The layout of ComplexStruct matches the slab's layout.
        let item1 = unsafe {
            slab.insert(ComplexStruct {
                a: 0x12,
                b: 0x3456,
                c: 0x789ABCDE,
                d: 0x123456789ABCDEF0,
                e: [0x11111111, 0x22222222, 0x33333333, 0x44444444],
                f: (0x5555, 0x66666666, 0x7777777777777777),
            })
        };

        // SAFETY: The layout of ComplexStruct matches the slab's layout.
        let item2 = unsafe {
            slab.insert(ComplexStruct {
                a: 0xAB,
                b: 0xCDEF,
                c: 0x12345678,
                d: 0xFEDCBA0987654321,
                e: [0xAAAAAAAA, 0xBBBBBBBB, 0xCCCCCCCC, 0xDDDDDDDD],
                f: (0xEEEE, 0xFFFFFFFF, 0x1111111111111111),
            })
        };

        unsafe {
            // Verify both can be read back correctly.
            let data1 = item1.ptr().read();
            assert_eq!(data1.a, 0x12);
            assert_eq!(data1.b, 0x3456);
            assert_eq!(data1.c, 0x789ABCDE);
            assert_eq!(data1.d, 0x123456789ABCDEF0);
            assert_eq!(data1.e, [0x11111111, 0x22222222, 0x33333333, 0x44444444]);
            assert_eq!(data1.f, (0x5555, 0x66666666, 0x7777777777777777));

            let data2 = item2.ptr().read();
            assert_eq!(data2.a, 0xAB);
            assert_eq!(data2.b, 0xCDEF);
            assert_eq!(data2.c, 0x12345678);
            assert_eq!(data2.d, 0xFEDCBA0987654321);
            assert_eq!(data2.e, [0xAAAAAAAA, 0xBBBBBBBB, 0xCCCCCCCC, 0xDDDDDDDD]);
            assert_eq!(data2.f, (0xEEEE, 0xFFFFFFFF, 0x1111111111111111));
        }

        slab.remove(item1.index());
        slab.remove(item2.index());
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

        let mut slab = OpaqueSlab::new(
            Layout::new::<TestEnum>(),
            NonZero::new(4).unwrap(),
            DropPolicy::MayDropItems,
        );

        // SAFETY: The layout of TestEnum matches the slab's layout.
        let item1 = unsafe { slab.insert(TestEnum::Variant1(0x12345678)) };
        // SAFETY: The layout of TestEnum matches the slab's layout.
        let item2 = unsafe {
            slab.insert(TestEnum::Variant2 {
                x: 0x1111111111111111,
                y: 0x2222222222222222,
            })
        };
        // SAFETY: The layout of TestEnum matches the slab's layout.
        let item3 = unsafe { slab.insert(TestEnum::Variant3) };

        unsafe {
            // Verify we can read them back (note: this is a bit unsafe since we're
            // treating the enum as raw memory, but it tests the layout handling).
            let _data1 = item1.ptr().as_ptr().read();
            let _data2 = item2.ptr().as_ptr().read();
            let _data3 = item3.ptr().as_ptr().read();
        }

        slab.remove(item1.index());
        slab.remove(item2.index());
        slab.remove(item3.index());
    }

    #[test]
    fn small_and_large_sizes() {
        // Test very small data types.
        let mut slab_small = OpaqueSlab::new(
            Layout::new::<u8>(),
            NonZero::new(100).unwrap(),
            DropPolicy::MayDropItems,
        );
        let mut items = Vec::new();

        // Fill with small values.
        for i in 0..100_u8 {
            // SAFETY: The layout of u8 matches the slab's layout.
            let item = unsafe { slab_small.insert(i) };
            items.push(item);
        }

        // Verify all values.
        for (i, item) in items.iter().enumerate() {
            unsafe {
                assert_eq!(
                    item.ptr().read(),
                    u8::try_from(i).expect("loop range is within u8 bounds")
                );
            }
        }

        // Clean up.
        for item in items {
            slab_small.remove(item.index());
        }

        // Test large data types.
        #[repr(C)]
        struct HugeStruct {
            data: [u64; 128], // 1024 bytes
        }

        let mut slab_large = OpaqueSlab::new(
            Layout::new::<HugeStruct>(),
            NonZero::new(5).unwrap(),
            DropPolicy::MayDropItems,
        );

        let mut huge_data = HugeStruct { data: [0; 128] };
        for (i, elem) in huge_data.data.iter_mut().enumerate() {
            *elem = (i as u64) * 0x0123456789ABCDEF;
        }

        // SAFETY: The layout of HugeStruct matches the slab's layout.
        let item = unsafe { slab_large.insert(huge_data) };

        unsafe {
            let read_data = item.ptr().read();
            for (i, &elem) in read_data.data.iter().enumerate() {
                assert_eq!(elem, (i as u64) * 0x0123456789ABCDEF);
            }
        }

        slab_large.remove(item.index());
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
    #[should_panic]
    fn layout_calculation_zero_size_panics() {
        let zero_layout = Layout::from_size_align(0, 1).unwrap();
        _ = SlabLayoutInfo::calculate(zero_layout, NonZero::new(3).unwrap());
    }

    #[test]
    #[should_panic]
    fn drop_with_active_items_panics() {
        let layout = Layout::new::<u32>();
        let mut slab = OpaqueSlab::new(
            layout,
            NonZero::new(3).unwrap(),
            DropPolicy::MustNotDropItems,
        );

        // Insert some item but don't remove it before drop.
        // SAFETY: The layout of u32 matches the slab's layout.
        let _item = unsafe { slab.insert(42_u32) };

        // When the slab goes out of scope and Drop is called, it should panic
        // because there's still an active item.
    }
}
