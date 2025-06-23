use std::alloc::Layout;
use std::ptr::NonNull;

/// Provides memory for ´CAPACITY` objects with a specific layout without knowing their type.
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
