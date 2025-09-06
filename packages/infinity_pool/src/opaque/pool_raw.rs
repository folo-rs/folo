use std::alloc::Layout;
use std::iter::{self, FusedIterator};
use std::mem::MaybeUninit;
use std::ptr::NonNull;

use crate::opaque::slab::SlabIterator;
use crate::{DropPolicy, RawOpaquePoolBuilder, RawPooled, RawPooledMut, Slab, SlabLayout};

// TODO: Factor out the vacancy cache into its own type.
// TODO: Optimize scenarios with mass inserts into a large reservation (slow today, poor vacancy cache logic).

/// A pool of objects with uniform memory layout.
///
/// Stores objects of any type that match a [`Layout`] defined at pool creation
/// time. All values in the pool remain pinned for their entire lifetime.
///
/// The pool automatically expands its capacity when needed.
#[doc = include_str!("../../doc/snippets/raw_pool_is_potentially_send.md")]
///
/// # Example
///
/// ```rust
/// use infinity_pool::RawOpaquePool;
///
/// fn work_with_displayable<T: std::fmt::Display + 'static + Unpin>(value: T) {
///     let mut pool = RawOpaquePool::with_layout_of::<T>();
///
///     // Insert an object into the pool
///     let handle = pool.insert(value);
///
///     // Access the object through the handle
///     let stored_value = unsafe { handle.ptr().as_ref() };
///     println!("Stored: {}", stored_value);
///
///     // Explicitly remove the object from the pool
///     pool.remove_mut(handle);
/// }
///
/// work_with_displayable("Hello, world!");
/// work_with_displayable(42);
/// ```
#[derive(Debug)]
pub struct RawOpaquePool {
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

impl RawOpaquePool {
    #[doc = include_str!("../../doc/snippets/pool_builder.md")]
    #[cfg_attr(test, mutants::skip)] // Gets mutated to alternate version of itself.
    pub fn builder() -> RawOpaquePoolBuilder {
        RawOpaquePoolBuilder::new()
    }

    /// Creates a new instance of the pool with the specified layout.
    ///
    /// Shorthand for a builder that keeps all other options at their default values.
    ///
    /// # Panics
    ///
    /// Panics if the layout is zero-sized.
    #[must_use]
    pub fn with_layout(object_layout: Layout) -> Self {
        Self::builder().layout(object_layout).build()
    }

    /// Creates a new instance of the pool with the layout of `T`.
    ///
    /// Shorthand for a builder that keeps all other options at their default values.
    ///
    /// # Panics
    ///
    /// Panics if `T` is a zero-sized type.
    #[must_use]
    pub fn with_layout_of<T: Sized>() -> Self {
        Self::builder().layout_of::<T>().build()
    }

    /// Creates a new pool for objects of the specified layout.
    ///
    /// # Panics
    ///
    /// Panics if the object layout has zero size.
    #[must_use]
    pub(crate) fn new_inner(object_layout: Layout, drop_policy: DropPolicy) -> Self {
        let slab_layout = SlabLayout::new(object_layout);

        Self {
            slab_layout,
            slabs: Vec::new(),
            slab_with_vacant_slot_index: None,
            drop_policy,
            length: 0,
        }
    }

    #[doc = include_str!("../../doc/snippets/opaque_pool_layout.md")]
    #[must_use]
    pub fn object_layout(&self) -> Layout {
        self.slab_layout.object_layout()
    }

    #[doc = include_str!("../../doc/snippets/pool_len.md")]
    #[must_use]
    pub fn len(&self) -> usize {
        self.length
    }

    #[doc = include_str!("../../doc/snippets/pool_capacity.md")]
    #[must_use]
    pub fn capacity(&self) -> usize {
        // Wrapping here would imply capacity is greater than virtual memory,
        // which is impossible because we can never create that many slabs.
        self.slabs
            .len()
            .wrapping_mul(self.slab_layout.capacity().get())
    }

    #[doc = include_str!("../../doc/snippets/pool_is_empty.md")]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    #[doc = include_str!("../../doc/snippets/pool_reserve.md")]
    pub fn reserve(&mut self, additional: usize) {
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

        self.slabs.extend(
            iter::repeat_with(|| Slab::new(self.slab_layout, self.drop_policy))
                .take(additional_slabs),
        );
    }

    #[doc = include_str!("../../doc/snippets/pool_shrink_to_fit.md")]
    #[cfg_attr(test, mutants::skip)] // Vacant slot cache mutation - hard to test. Revisit later.
    pub fn shrink_to_fit(&mut self) {
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
        // since it might point to a slab that will no longer exist.
        if new_len < self.slabs.len() {
            self.slab_with_vacant_slot_index = None;
        }

        // Truncate the slabs vector to remove empty slabs from the end.
        self.slabs.truncate(new_len);
    }

    #[doc = include_str!("../../doc/snippets/pool_insert.md")]
    ///
    /// # Panics
    #[doc = include_str!("../../doc/snippets/panic_on_pool_t_layout_mismatch.md")]
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use infinity_pool::RawOpaquePool;
    ///
    /// let mut pool = RawOpaquePool::with_layout(Layout::new::<String>());
    ///
    /// // Insert an object into the pool
    /// let mut handle = pool.insert("Hello".to_string());
    ///
    /// // Mutate the object via the unique handle
    /// // SAFETY: The handle is valid and points to a properly initialized String
    /// unsafe {
    ///     handle.as_mut().push_str(", Raw Opaque World!");
    ///     assert_eq!(handle.as_ref(), "Hello, Raw Opaque World!");
    /// }
    ///
    /// // Transform the unique handle into a shared handle
    /// let shared_handle = handle.into_shared();
    ///
    /// // After transformation, you can only immutably dereference the object
    /// // SAFETY: The shared handle is valid and points to a properly initialized String
    /// unsafe {
    ///     assert_eq!(shared_handle.as_ref(), "Hello, Raw Opaque World!");
    ///     // shared_handle.as_mut(); // This would not compile
    /// }
    ///
    /// // Explicitly remove the object from the pool
    /// // SAFETY: The handle belongs to this pool and references a valid object
    /// unsafe {
    ///     pool.remove(shared_handle);
    /// }
    /// assert_eq!(pool.len(), 0);
    /// ```
    pub fn insert<T>(&mut self, value: T) -> RawPooledMut<T> {
        assert_eq!(
            Layout::new::<T>(),
            self.object_layout(),
            "layout of T does not match object layout of the pool"
        );

        // SAFETY: We just verified that T's layout matches the pool's layout.
        unsafe { self.insert_unchecked(value) }
    }

    #[doc = include_str!("../../doc/snippets/pool_insert.md")]
    /// # Safety
    #[doc = include_str!("../../doc/snippets/safety_pool_t_layout_must_match.md")]
    pub unsafe fn insert_unchecked<T>(&mut self, value: T) -> RawPooledMut<T> {
        // Implement insert() in terms of insert_with() to reduce logic duplication.
        // SAFETY: Forwarding safety requirements to the caller.
        unsafe {
            self.insert_with_unchecked(|uninit: &mut MaybeUninit<T>| {
                uninit.write(value);
            })
        }
    }

    #[doc = include_str!("../../doc/snippets/pool_insert_with.md")]
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::mem::MaybeUninit;
    ///
    /// use infinity_pool::RawOpaquePool;
    ///
    /// struct DataBuffer {
    ///     id: u32,
    ///     data: MaybeUninit<[u8; 1024]>, // Large buffer to skip initializing
    /// }
    ///
    /// let mut pool = RawOpaquePool::with_layout_of::<DataBuffer>();
    ///
    /// // Initialize only the id, leaving data uninitialized for performance
    /// let handle = unsafe {
    ///     pool.insert_with(|uninit: &mut MaybeUninit<DataBuffer>| {
    ///         let ptr = uninit.as_mut_ptr();
    ///         // SAFETY: Writing to the id field within allocated space
    ///         unsafe {
    ///             std::ptr::addr_of_mut!((*ptr).id).write(42);
    ///             // data field is intentionally left uninitialized
    ///         }
    ///     })
    /// };
    ///
    /// // ID is accessible, data remains uninitialized
    /// let id = unsafe { std::ptr::addr_of!(handle.ptr().as_ref().id).read() };
    /// assert_eq!(id, 42);
    /// ```
    ///
    /// # Panics
    #[doc = include_str!("../../doc/snippets/panic_on_pool_t_layout_mismatch.md")]
    ///
    /// # Safety
    #[doc = include_str!("../../doc/snippets/safety_closure_must_initialize_object.md")]
    pub unsafe fn insert_with<T, F>(&mut self, f: F) -> RawPooledMut<T>
    where
        F: FnOnce(&mut MaybeUninit<T>),
    {
        assert_eq!(
            Layout::new::<T>(),
            self.object_layout(),
            "layout of T does not match object layout of the pool"
        );

        // SAFETY: We just verified that T's layout matches the pool's layout.
        unsafe { self.insert_with_unchecked(f) }
    }

    #[doc = include_str!("../../doc/snippets/pool_insert_with.md")]
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::mem::MaybeUninit;
    ///
    /// use infinity_pool::RawOpaquePool;
    ///
    /// struct DataBuffer {
    ///     id: u32,
    ///     data: MaybeUninit<[u8; 1024]>, // Large buffer to skip initializing
    /// }
    ///
    /// let mut pool = RawOpaquePool::with_layout_of::<DataBuffer>();
    ///
    /// // Initialize only the id, leaving data uninitialized for performance
    /// let handle = unsafe {
    ///     pool.insert_with_unchecked(|uninit: &mut MaybeUninit<DataBuffer>| {
    ///         let ptr = uninit.as_mut_ptr();
    ///         // SAFETY: Writing to the id field within allocated space
    ///         unsafe {
    ///             std::ptr::addr_of_mut!((*ptr).id).write(42);
    ///             // data field is intentionally left uninitialized
    ///         }
    ///     })
    /// };
    ///
    /// // ID is accessible, data remains uninitialized
    /// let id = unsafe { std::ptr::addr_of!(handle.ptr().as_ref().id).read() };
    /// assert_eq!(id, 42);
    /// ```
    ///
    /// # Safety
    #[doc = include_str!("../../doc/snippets/safety_pool_t_layout_must_match.md")]
    #[doc = include_str!("../../doc/snippets/safety_closure_must_initialize_object.md")]
    pub unsafe fn insert_with_unchecked<T, F>(&mut self, f: F) -> RawPooledMut<T>
    where
        F: FnOnce(&mut MaybeUninit<T>),
    {
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

        // Update our tracked length since we just inserted an object.
        // This can never overflow since that would mean the pool is greater than virtual memory.
        self.length = self.length.wrapping_add(1);

        // The pool itself does not care about the type T but for the convenience of the caller
        // we imbue the RawPooledMut with the type information, to reduce required casting by caller.
        RawPooledMut::new(slab_index, slab_handle)
    }

    #[doc = include_str!("../../doc/snippets/raw_pool_remove_mut.md")]
    pub fn remove_mut<T: ?Sized>(&mut self, handle: RawPooledMut<T>) {
        // SAFETY: The provided handle is a unique handle, which guarantees that the object
        // has not been removed yet (because doing so consumes the unique handle).
        unsafe {
            self.remove(handle.into_shared());
        }
    }

    #[doc = include_str!("../../doc/snippets/raw_pool_remove.md")]
    pub unsafe fn remove<T: ?Sized>(&mut self, handle: RawPooled<T>) {
        let slab = self
            .slabs
            .get_mut(handle.slab_index())
            .expect("the RawPooled did not point to an object in this pool");

        // SAFETY: Forwarding guarantees from caller.
        unsafe {
            slab.remove(handle.slab_handle());
        }

        // Update our tracked length since we just removed an object.
        // This cannot wrap around because we just removed an object,
        // so the value must be at least 1 before subtraction.
        self.length = self.length.wrapping_sub(1);

        // There is now a vacant slot in this slab! We remember this for fast insertions.
        // We try to remember the lowest index of a slab with a vacant slot, so we
        // fill the collection from the start (to enable easier shrinking later).
        self.update_vacant_slot_cache(handle.slab_index());
    }

    #[doc = include_str!("../../doc/snippets/raw_pool_remove_mut_unpin.md")]
    #[must_use]
    pub fn remove_mut_unpin<T: Unpin>(&mut self, handle: RawPooledMut<T>) -> T {
        // SAFETY: The provided handle is a unique handle, which guarantees that the object
        // has not been removed yet (because doing so consumes the unique handle).
        unsafe { self.remove_unpin(handle.into_shared()) }
    }

    #[doc = include_str!("../../doc/snippets/raw_pool_remove_unpin.md")]
    #[must_use]
    pub unsafe fn remove_unpin<T: Unpin>(&mut self, handle: RawPooled<T>) -> T {
        // We would rather prefer to check for `RawPooled<()>` specifically but
        // that would imply specialization or `T: 'static` or TypeId shenanigans.
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
            .expect("the RawPooled did not point to an existing object in the pool");

        // SAFETY: The RawPooled<T> guarantees the type T is correct for this pool slot.
        let value = unsafe { slab.remove_unpin::<T>(handle.slab_handle()) };

        // Update our tracked length since we just removed an object.
        // This cannot wrap around because we just removed an object,
        // so the value must be at least 1 before subtraction.
        self.length = self.length.wrapping_sub(1);

        // There is now a vacant slot in this slab! We remember this for fast insertions.
        // We try to remember the lowest index of a slab with a vacant slot, so we
        // fill the collection from the start (to enable easier shrinking later).
        self.update_vacant_slot_cache(handle.slab_index());

        value
    }

    #[doc = include_str!("../../doc/snippets/raw_pool_iter.md")]
    #[must_use]
    pub fn iter(&self) -> RawOpaquePoolIterator<'_> {
        RawOpaquePoolIterator::new(self)
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

/// Iterator over all objects in a raw opaque pool.
///
/// This iterator yields untyped pointers to objects stored across all slabs in the pool.
/// Since the pool can contain objects of different types (as long as they have the same layout),
/// the iterator returns `NonNull<()>` and leaves type casting to the caller.
///
/// # Thread safety
///
/// The type is single-threaded.
#[derive(Debug)]
pub struct RawOpaquePoolIterator<'p> {
    pool: &'p RawOpaquePool,

    // Current slab index for forward iteration.
    // This is the index of the next slab we will take items from.
    // If iterator is exhausted, will point to undefined value.
    current_front_slab_index: usize,

    // Current slab index for backward iteration.
    // This is the index of the next slab we will take items from.
    // If iterator is exhausted, will point to undefined value.
    current_back_slab_index: usize,

    // Iterator for the current front slab (if any).
    current_front_slab_iter: Option<SlabIterator<'p>>,

    // Iterator for the current back slab (if any).
    current_back_slab_iter: Option<SlabIterator<'p>>,

    // Total number of items already yielded.
    yielded_count: usize,
}

impl<'p> RawOpaquePoolIterator<'p> {
    fn new(pool: &'p RawOpaquePool) -> Self {
        Self {
            pool,
            current_front_slab_index: 0,
            // This is allowed to wrap - if the iterator is exhausted, we point to undefined value.
            current_back_slab_index: pool.slabs.len().wrapping_sub(1),
            current_front_slab_iter: None,
            current_back_slab_iter: None,
            yielded_count: 0,
        }
    }
}

impl Iterator for RawOpaquePoolIterator<'_> {
    type Item = NonNull<()>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.len() > 0 {
            // If no current iterator, get one for the current slab.
            let slab_iter = self.current_front_slab_iter.get_or_insert_with(|| {
                self.pool.slabs
                    .get(self.current_front_slab_index)
                    .expect("iterator has items remaining, so there must still be a slab to get them from")
                    .iter()
            });

            // Try to get the next item from current iterator
            if let Some(item) = slab_iter.next() {
                // Will never wrap because that would mean we have more
                // items than we have virtual memory.
                self.yielded_count = self.yielded_count.wrapping_add(1);
                return Some(item);
            }

            // No more items from this slab, move to next
            // This is allowed to wrap - if the iterator is exhausted, we point to undefined value.
            self.current_front_slab_index = self.current_front_slab_index.wrapping_add(1);
            self.current_front_slab_iter = None;
        }

        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.len();
        (remaining, Some(remaining))
    }
}

impl DoubleEndedIterator for RawOpaquePoolIterator<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        while self.len() > 0 {
            // If no current iterator, get one for the current slab.
            let slab_iter = self.current_back_slab_iter.get_or_insert_with(|| {
                self.pool.slabs
                    .get(self.current_back_slab_index)
                    .expect("iterator has items remaining, so there must still be a slab to get them from")
                    .iter()
            });

            // Try to get the next item from current iterator
            if let Some(item) = slab_iter.next_back() {
                // Will never wrap because that would mean we have more
                // items than we have virtual memory.
                self.yielded_count = self.yielded_count.wrapping_add(1);
                return Some(item);
            }

            // No more items from this slab, move to next
            // This is allowed to wrap - if the iterator is exhausted, we point to undefined value.
            self.current_back_slab_index = self.current_back_slab_index.wrapping_sub(1);
            self.current_back_slab_iter = None;
        }

        None
    }
}

impl ExactSizeIterator for RawOpaquePoolIterator<'_> {
    fn len(&self) -> usize {
        // Total objects in pool minus those we've already yielded
        // Will not wrap because we cannot yield more items than exist in the pool.
        self.pool.len().wrapping_sub(self.yielded_count)
    }
}

// Once we return None, we will keep returning None.
impl FusedIterator for RawOpaquePoolIterator<'_> {}

impl<'p> IntoIterator for &'p RawOpaquePool {
    type Item = NonNull<()>;
    type IntoIter = RawOpaquePoolIterator<'p>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
#[allow(
    clippy::indexing_slicing,
    clippy::multiple_unsafe_ops_per_block,
    clippy::undocumented_unsafe_blocks,
    reason = "tests focus on succinct code and do not need to tick all the boxes"
)]
mod tests {
    use std::alloc::Layout;
    use std::mem::MaybeUninit;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(RawOpaquePoolIterator<'_>: Iterator, DoubleEndedIterator, ExactSizeIterator, FusedIterator);
    assert_not_impl_any!(RawOpaquePoolIterator<'_>: Send, Sync);

    assert_impl_all!(&RawOpaquePool: IntoIterator);

    #[test]
    fn new_pool_is_empty() {
        let pool = RawOpaquePool::with_layout_of::<u64>();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity(), 0);
        assert_eq!(pool.object_layout(), Layout::new::<u64>());
    }

    #[test]
    fn with_layout_results_in_pool_with_correct_layout() {
        let layout = Layout::new::<i64>();
        let pool = RawOpaquePool::with_layout(layout);

        assert_eq!(pool.object_layout(), layout);
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity(), 0);
    }

    #[test]
    fn instance_creation_through_builder_succeeds() {
        let pool = RawOpaquePool::builder()
            .layout_of::<i64>()
            .drop_policy(DropPolicy::MustNotDropContents)
            .build();

        assert_eq!(pool.object_layout(), Layout::new::<i64>());
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity(), 0);
    }

    #[test]
    fn insert_and_length() {
        let mut pool = RawOpaquePool::with_layout_of::<u32>();

        let _handle1 = pool.insert(42_u32);
        assert_eq!(pool.len(), 1);
        assert!(!pool.is_empty());

        let _handle2 = pool.insert(100_u32);
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn capacity_grows_with_slabs() {
        let mut pool = RawOpaquePool::with_layout_of::<u64>();

        assert_eq!(pool.capacity(), 0);

        let _handle = pool.insert(123_u64);

        // Should have at least one slab's worth of capacity now
        assert!(pool.capacity() > 0);
        let initial_capacity = pool.capacity();

        // Fill up the slab to force creation of a new one
        for i in 1..initial_capacity {
            let _handle = pool.insert(i as u64);
        }

        // One more insert should create a new slab
        let _handle = pool.insert(999_u64);

        assert!(pool.capacity() >= initial_capacity * 2);
    }

    #[test]
    fn reserve_creates_capacity() {
        let mut pool = RawOpaquePool::with_layout_of::<u8>();

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
        let mut pool = RawOpaquePool::with_layout_of::<u64>();

        let handle = unsafe {
            pool.insert_with(|uninit: &mut MaybeUninit<u64>| {
                uninit.write(42);
            })
        };

        assert_eq!(pool.len(), 1);

        let value = pool.remove_mut_unpin(handle);
        assert_eq!(value, 42);
    }

    #[test]
    fn remove_decreases_length() {
        let mut pool = RawOpaquePool::with_layout_of::<String>();

        let handle1 = pool.insert("hello".to_string());
        let handle2 = pool.insert("world".to_string());

        assert_eq!(pool.len(), 2);

        pool.remove_mut(handle1);
        assert_eq!(pool.len(), 1);

        pool.remove_mut(handle2);
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn remove_unpin_returns_value() {
        let mut pool = RawOpaquePool::with_layout_of::<i32>();

        let handle = pool.insert(-456_i32);

        let value = pool.remove_mut_unpin(handle);
        assert_eq!(value, -456);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn shrink_to_fit_removes_empty_slabs() {
        let mut pool = RawOpaquePool::with_layout_of::<u8>();

        // Add some items.
        let mut handles = Vec::new();
        for i in 0..10 {
            handles.push(pool.insert(u8::try_from(i).unwrap()));
        }

        // Remove all items.
        for handle in handles {
            pool.remove_mut(handle);
        }

        assert!(pool.is_empty());

        pool.shrink_to_fit();

        // We have white-box knowledge that an empty pool will shrink to zero.
        // This may become untrue with future algorithm changes, at which point
        // we will need to adjust the tests.
        assert_eq!(pool.capacity(), 0);
    }

    #[test]
    fn handle_provides_access_to_object() {
        let mut pool = RawOpaquePool::with_layout_of::<u64>();

        let handle = pool.insert(12345_u64);

        assert_eq!(unsafe { *handle.as_ref() }, 12345);

        // Access the value through the handle's pointer
        let ptr = handle.ptr();

        let value = unsafe { ptr.as_ref() };

        assert_eq!(*value, 12345);
    }

    #[test]
    fn shared_handles_are_copyable() {
        let mut pool = RawOpaquePool::with_layout_of::<u32>();

        let handle1 = pool.insert(789_u32).into_shared();
        let handle2 = handle1;
        #[expect(clippy::clone_on_copy, reason = "intentional, testing cloning")]
        let handle3 = handle1.clone();

        unsafe {
            assert_eq!(*handle1.as_ref(), *handle2.as_ref());
            assert_eq!(*handle1.as_ref(), *handle3.as_ref());
            assert_eq!(*handle2.as_ref(), *handle3.as_ref());
        }
    }

    #[test]
    fn multiple_removals_and_insertions() {
        let mut pool = RawOpaquePool::with_layout_of::<usize>();

        // Insert, remove, insert again to test slot reuse
        let handle1 = pool.insert(1_usize);
        pool.remove_mut(handle1);

        let handle2 = pool.insert(2_usize);

        assert_eq!(pool.len(), 1);

        let value = pool.remove_mut_unpin(handle2);
        assert_eq!(value, 2);
    }

    #[test]
    fn remove_with_shared_handle() {
        let mut pool = RawOpaquePool::with_layout_of::<i64>();

        let handle = pool.insert(999_i64).into_shared();

        assert_eq!(pool.len(), 1);

        unsafe {
            pool.remove(handle);
        }

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn remove_unpin_with_shared_handle() {
        let mut pool = RawOpaquePool::with_layout_of::<i32>();

        let handle = pool.insert(42_i32).into_shared();

        assert_eq!(pool.len(), 1);

        let value = unsafe { pool.remove_unpin(handle) };

        assert_eq!(value, 42);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    #[should_panic]
    fn remove_unpin_panics_on_zero_sized_type() {
        // We need to use a type that is not zero-sized for the pool itself,
        // but we create a handle that gets type-erased to a ZST.
        let mut pool = RawOpaquePool::with_layout_of::<u8>();

        let handle = pool.insert(123_u8);

        let erased_handle: RawPooled<()> = handle.into_shared().erase();

        // This should panic because size_of::<()>() == 0
        unsafe {
            #[expect(unused_must_use, reason = "impossible to use a unit value")]
            pool.remove_unpin(erased_handle);
        }
    }

    #[test]
    #[should_panic]
    fn insert_panics_if_provided_type_with_wrong_layout() {
        let mut pool = RawOpaquePool::with_layout_of::<u32>();

        // Try to insert a u64 into a pool configured for u32
        let _handle = pool.insert(123_u64);
    }

    #[test]
    #[should_panic]
    fn insert_with_panics_if_provided_type_with_wrong_layout() {
        let mut pool = RawOpaquePool::with_layout_of::<u16>();

        // Try to insert a u32 into a pool configured for u16
        let _handle = unsafe {
            pool.insert_with(|uninit: &mut MaybeUninit<u32>| {
                uninit.write(456);
            })
        };
    }

    #[test]
    fn iter_empty_pool() {
        let pool = RawOpaquePool::with_layout_of::<u32>();

        let mut iter = pool.iter();
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert_eq!(iter.len(), 0);

        assert_eq!(iter.next(), None);
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert_eq!(iter.len(), 0);
    }

    #[test]
    fn iter_single_item() {
        let mut pool = RawOpaquePool::with_layout_of::<u32>();

        let _handle = pool.insert(42_u32);

        let mut iter = pool.iter();
        assert_eq!(iter.len(), 1);

        // First item should be the object we inserted
        let ptr = iter.next().expect("should have one item");

        let value = unsafe { ptr.cast::<u32>().as_ref() };
        assert_eq!(*value, 42);

        // No more items
        assert_eq!(iter.next(), None);
        assert_eq!(iter.len(), 0);
    }

    #[test]
    fn iter_multiple_items_single_slab() {
        let mut pool = RawOpaquePool::with_layout_of::<u32>();

        // Insert multiple items that should fit in a single slab
        let _handle1 = pool.insert(100_u32);
        let _handle2 = pool.insert(200_u32);
        let _handle3 = pool.insert(300_u32);

        let values: Vec<u32> = pool
            .iter()
            .map(|ptr| unsafe { *ptr.cast::<u32>().as_ref() })
            .collect();

        // Should get all values in order of their slot indices
        assert_eq!(values, vec![100, 200, 300]);
    }

    #[test]
    fn iter_multiple_items_multiple_slabs() {
        let mut pool = RawOpaquePool::with_layout_of::<u8>();

        // Insert enough items to span multiple slabs
        #[allow(
            clippy::collection_is_never_read,
            reason = "handles are used for ownership"
        )]
        let mut handles = Vec::new();
        for i in 0..50 {
            handles.push(pool.insert(u8::try_from(i).unwrap()));
        }

        let values: Vec<u8> = pool
            .iter()
            .map(|ptr| unsafe { *ptr.cast::<u8>().as_ref() })
            .collect();

        // Should get all values we inserted
        assert_eq!(values.len(), 50);
        for (i, &value) in values.iter().enumerate() {
            assert_eq!(value, u8::try_from(i).unwrap());
        }

        // Clean up
        for handle in handles {
            pool.remove_mut(handle);
        }
    }

    #[test]
    fn iter_with_gaps() {
        let mut pool = RawOpaquePool::with_layout_of::<u32>();

        // Insert items
        let _handle1 = pool.insert(100_u32);
        let handle2 = pool.insert(200_u32);
        let _handle3 = pool.insert(300_u32);

        // Remove the middle item to create a gap
        pool.remove_mut(handle2);

        let values: Vec<u32> = pool
            .iter()
            .map(|ptr| unsafe { *ptr.cast::<u32>().as_ref() })
            .collect();

        // Should get only the remaining values
        assert_eq!(values, vec![100, 300]);
    }

    #[test]
    fn iter_with_empty_slabs() {
        let mut pool = RawOpaquePool::with_layout_of::<u64>();

        // Force creation of multiple slabs by inserting many items
        let mut handles = Vec::new();
        for i in 0_u64..20 {
            handles.push(pool.insert(i));
        }

        // Remove all items from some slabs to create empty slabs
        for handle in handles.drain(5..15) {
            pool.remove_mut(handle);
        }

        let values: Vec<u64> = pool
            .iter()
            .map(|ptr| unsafe { *ptr.cast::<u64>().as_ref() })
            .collect();

        // Should get values from non-empty slabs only
        let expected: Vec<u64> = (0..5_u64).chain(15..20_u64).collect();
        assert_eq!(values, expected);
    }

    #[test]
    fn iter_size_hint() {
        let mut pool = RawOpaquePool::with_layout_of::<u32>();

        // Empty pool
        let iter = pool.iter();
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert_eq!(iter.len(), 0);

        // Add some items
        let _handle1 = pool.insert(100_u32);
        let _handle2 = pool.insert(200_u32);

        let mut iter = pool.iter();
        assert_eq!(iter.size_hint(), (2, Some(2)));
        assert_eq!(iter.len(), 2);

        // Consume one item
        let first_item = iter.next();
        assert!(first_item.is_some());
        assert_eq!(iter.size_hint(), (1, Some(1)));
        assert_eq!(iter.len(), 1);

        // Consume another
        let second_item = iter.next();
        assert!(second_item.is_some());
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert_eq!(iter.len(), 0);

        // Should be exhausted now
        assert_eq!(iter.next(), None);
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert_eq!(iter.len(), 0);
    }

    #[test]
    fn iter_double_ended_basic() {
        let mut pool = RawOpaquePool::with_layout_of::<u32>();

        // Insert items
        let _handle1 = pool.insert(100_u32);
        let _handle2 = pool.insert(200_u32);
        let _handle3 = pool.insert(300_u32);

        let mut iter = pool.iter();

        // Iterate from the back
        let last_ptr = iter.next_back().expect("should have last item");
        let last_value = unsafe { *last_ptr.cast::<u32>().as_ref() };
        assert_eq!(last_value, 300);

        let middle_ptr = iter.next_back().expect("should have middle item");
        let middle_value = unsafe { *middle_ptr.cast::<u32>().as_ref() };
        assert_eq!(middle_value, 200);

        let first_ptr = iter.next_back().expect("should have first item");
        let first_value = unsafe { *first_ptr.cast::<u32>().as_ref() };
        assert_eq!(first_value, 100);

        // Should be exhausted
        assert_eq!(iter.next_back(), None);
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn iter_double_ended_mixed_directions() {
        let mut pool = RawOpaquePool::with_layout_of::<u32>();

        // Insert 5 items
        let _handle1 = pool.insert(100_u32);
        let _handle2 = pool.insert(200_u32);
        let _handle3 = pool.insert(300_u32);
        let _handle4 = pool.insert(400_u32);
        let _handle5 = pool.insert(500_u32);

        let mut iter = pool.iter();
        assert_eq!(iter.len(), 5);

        // Get first from front
        let first_ptr = iter.next().expect("should have first item");
        let first_value = unsafe { *first_ptr.cast::<u32>().as_ref() };
        assert_eq!(first_value, 100);
        assert_eq!(iter.len(), 4);

        // Get last from back
        let last_ptr = iter.next_back().expect("should have last item");
        let last_value = unsafe { *last_ptr.cast::<u32>().as_ref() };
        assert_eq!(last_value, 500);
        assert_eq!(iter.len(), 3);

        // Get second from front
        let second_ptr = iter.next().expect("should have second item");
        let second_value = unsafe { *second_ptr.cast::<u32>().as_ref() };
        assert_eq!(second_value, 200);
        assert_eq!(iter.len(), 2);

        // Get fourth from back
        let fourth_ptr = iter.next_back().expect("should have fourth item");
        let fourth_value = unsafe { *fourth_ptr.cast::<u32>().as_ref() };
        assert_eq!(fourth_value, 400);
        assert_eq!(iter.len(), 1);

        // Get middle item
        let middle_ptr = iter.next().expect("should have middle item");
        let middle_value = unsafe { *middle_ptr.cast::<u32>().as_ref() };
        assert_eq!(middle_value, 300);
        assert_eq!(iter.len(), 0);

        // Should be exhausted
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next_back(), None);
        assert_eq!(iter.len(), 0);
    }

    #[test]
    fn iter_fused_behavior() {
        let mut pool = RawOpaquePool::with_layout_of::<u32>();

        // Test with empty pool
        let mut iter = pool.iter();
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next(), None); // Should still be None
        assert_eq!(iter.next_back(), None);
        assert_eq!(iter.next_back(), None); // Should still be None

        // Test with some items
        let _handle1 = pool.insert(100_u32);
        let _handle2 = pool.insert(200_u32);

        let mut iter = pool.iter();

        // Consume all items
        let first = iter.next();
        assert!(first.is_some());
        let second = iter.next();
        assert!(second.is_some());

        // Now iterator should be exhausted
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next(), None); // FusedIterator guarantee: still None
        assert_eq!(iter.next(), None); // Still None
        assert_eq!(iter.next_back(), None); // Should also be None from back
        assert_eq!(iter.next_back(), None); // Still None from back

        // Test bidirectional exhaustion
        let mut iter = pool.iter();

        // Consume from both ends until exhausted
        iter.next(); // Consume from front
        iter.next_back(); // Consume from back

        // Now should be exhausted
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next_back(), None);
        assert_eq!(iter.next(), None); // FusedIterator guarantee
        assert_eq!(iter.next_back(), None); // FusedIterator guarantee
    }

    #[test]
    fn iter_across_multiple_slabs_with_gaps() {
        let mut pool = RawOpaquePool::with_layout_of::<usize>();

        // Create a pattern: insert many items, remove some to create gaps across slabs
        let mut handles = Vec::new();
        for i in 0_usize..30 {
            handles.push(pool.insert(i));
        }

        // Remove every third item to create gaps across slabs
        let mut to_remove = Vec::new();
        for (index, _) in handles.iter().enumerate().step_by(3) {
            to_remove.push(index);
        }

        // Remove in reverse order to maintain indices
        for &index in to_remove.iter().rev() {
            pool.remove_mut(handles.swap_remove(index));
        }

        let values: Vec<usize> = pool
            .iter()
            .map(|ptr| unsafe { *ptr.cast::<usize>().as_ref() })
            .collect();

        // Should get all non-removed values
        let expected: Vec<usize> = (0_usize..30).filter(|&i| i % 3 != 0).collect();
        assert_eq!(values, expected);
    }

    #[test]
    fn into_iterator_trait_works() {
        let mut pool = RawOpaquePool::with_layout_of::<u32>();

        let _handle1 = pool.insert(100_u32);
        let _handle2 = pool.insert(200_u32);
        let _handle3 = pool.insert(300_u32);

        // Test using for-in loop (which uses IntoIterator)
        let mut values = Vec::new();
        for ptr in &pool {
            let value = unsafe { *ptr.cast::<u32>().as_ref() };
            values.push(value);
        }

        assert_eq!(values, vec![100, 200, 300]);
    }
}
