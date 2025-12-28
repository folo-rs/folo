use std::alloc::{Layout, alloc, dealloc};
use std::any::{Any, type_name};
use std::iter::FusedIterator;
use std::mem::{self, MaybeUninit, size_of};
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::ptr::{self, NonNull};
use std::thread;

use crate::{DropPolicy, Dropper, SlabHandle, SlabLayout, SlotMeta};

/// A slab is one piece of an object pool's capacity.
///
/// Inserting an object into the slab returns a handle that can be used to access or remove the
/// object.
///
/// # Out of band access
///
/// The slab does not create or keep references to the objects within, so it is valid to access
/// memory via pointers and to create exclusive references to objects in the slab from unsafe code
/// even when not holding an exclusive reference to the slab.
///
/// The caller is responsible for ensuring that no reference survives after the object is removed
/// from the slab or the slab is dropped. Higher-level abstractions in this package provide safe
/// wrappers around these unsafe lifetime management principles.
///
/// # Thread safety
///
/// The slab is single-threaded by default, though if all the objects inserted are `Send` then
/// the owner of the slab is allowed to treat the slab itself as `Send` (but must do so via a
/// wrapper type that implements `Send` using unsafe code).
#[derive(Debug)]
pub(crate) struct Slab {
    /// Precomputed layout factors for the slab, based on the object layout and capacity.
    layout: SlabLayout,

    /// Drop policy that determines whether the slab panics if any objects
    /// are present in the slab when the slab is dropped.
    drop_policy: DropPolicy,

    /// Base pointer for the array containing all slots in the slab, where each slot combines
    /// metadata and object storage. Each slot is an untyped pseudo-object with a structure
    /// equivalent to this Rust type:
    ///
    /// ```ignore
    /// struct Slot {
    ///     meta: SlotMeta,
    ///     object: <a type with object_layout>,
    /// }
    /// ```
    ///
    /// The layout of this pseudo-object, the layout of the array, and the offset
    /// to the `object` are all provided by the data in the `slab_layout` field.
    first_slot_ptr: NonNull<SlotMeta>,

    /// Head of the intrusive freelist implementing a stack of available slots, where each
    /// vacant slot stores the index of the next vacant slot. Points beyond capacity when full.
    next_free_slot_index: usize,

    /// Current number of occupied slots.
    count: usize,
}

impl Slab {
    /// Creates a new slab with the specified layout.
    #[must_use]
    pub(crate) fn new(layout: SlabLayout, drop_policy: DropPolicy) -> Self {
        // SAFETY: SlabLayout guarantees we have a valid layout here.
        let first_slot_ptr = NonNull::new(unsafe { alloc(layout.slot_array_layout()) })
            .expect("allocation failure is not recoverable")
            .cast::<SlotMeta>();

        ensure_virtual_pages_mapped_to_physical_pages(first_slot_ptr, &layout);

        // We initialize all the slots to "vacant" to start with.
        for index in 0_usize..layout.capacity().get() {
            // Cannot overflow because that would imply slab extends beyond virtual memory,
            // which would have failed at the layout generation stage above.
            let slot_offset_bytes = index.wrapping_mul(layout.slot_layout().size());

            // SAFETY: Index is bounded by loop conditions and the condition itself is supplied
            // by the slab layout, so we are guaranteed to be in-bounds.
            let slot_ptr = unsafe { first_slot_ptr.as_ptr().cast::<u8>().add(slot_offset_bytes) };

            #[expect(
                clippy::cast_ptr_alignment,
                reason = "SlabLayout guarantees proper alignment"
            )]
            let slot_meta_ptr = slot_ptr.cast::<SlotMeta>();

            // SAFETY: The layout guarantees that this memory is valid for writes of SlotMeta,
            // and we are the only owner of this memory, which we just allocated above.
            unsafe {
                ptr::write(
                    slot_meta_ptr,
                    SlotMeta::Vacant {
                        // Cannot overflow, as that would imply the slab is larger than virtual
                        // memory, which would have failed to generate a layout.
                        next_free_slot_index: index.wrapping_add(1),
                    },
                );
            }
        }

        Self {
            layout,
            drop_policy,
            first_slot_ptr,
            next_free_slot_index: 0,
            count: 0,
        }
    }

    /// The number of objects in the slab.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.count
    }

    /// Whether the slab contains no objects.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Whether the slab is at capacity and no more objects can be inserted.
    #[must_use]
    pub(crate) fn is_full(&self) -> bool {
        self.count == self.layout.capacity().get()
    }

    /// Inserts an object into the slab via closure.
    ///
    /// This method allows the caller to partially initialize the object, skipping any `MaybeUninit`
    /// fields that are intentionally not initialized at insertion time. This can make insertion of
    /// objects containing `MaybeUninit` fields faster, although requires unsafe code to implement.
    ///
    /// This method is NOT faster than `insert()` for fully initialized objects. Use `insert()`
    /// for a better safety posture if you do not intend to skip initialization of any
    /// `MaybeUninit` fields.
    ///
    /// The returned handle can be used to access the object and remove it from the slab.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the layout of `T` matches the slab's object layout.
    ///
    /// The caller must ensure that the closure correctly initializes the object. All fields that
    /// are not `MaybeUninit` must be initialized when the closure returns.
    ///
    /// The caller must ensure that the slab is not full. There is no bounds checking performed.
    pub(crate) unsafe fn insert_with_unchecked<T, F>(&mut self, f: F) -> SlabHandle<T>
    where
        F: FnOnce(&mut MaybeUninit<T>),
    {
        debug_assert_eq!(
            Layout::new::<T>(),
            self.layout.object_layout(),
            "type {} layout mismatch: expected layout {:?}, got layout {:?}",
            type_name::<T>(),
            self.layout.object_layout(),
            Layout::new::<T>()
        );

        debug_assert!(
            !self.is_full(),
            "cannot insert value into a full Slab<{}> of capacity {}",
            type_name::<T>(),
            self.layout.capacity().get()
        );

        // Pop the next free index from the stack of free entries.
        let index = self.next_free_slot_index;

        // SAFETY: Guaranteed in-bounds since we have a safety requirement that the slab is not
        // full and we picked the next free slot index from the freelist.
        let mut slot_ptr = unsafe { self.slot_ptr_unchecked(index) };

        // SAFETY: We hold an exclusive reference to the slab (&mut self), and slot_ptr
        // points to a valid, initialized SlotMeta. Access to SlotMeta objects requires
        // exclusive access to the slab, so this is safe.
        let slot_meta = unsafe { slot_ptr.as_mut() };

        // SAFETY: Guaranteed in-bounds since we have a safety requirement that the slab is not
        // full and we picked the next free slot index from the freelist.
        let object_ptr = unsafe { self.object_ptr_unchecked::<T>(index) };

        // We create a MaybeUninit wrapper around the memory location where the object is to be
        // stored and call the initialization function to fill any part of it that need filling
        // to consider it initialized (this may skip `MaybeUninit` fields that do not need to be
        // initialized for the object to be considered initialized).
        {
            // SAFETY: object_ptr points to valid, properly aligned memory for type T within our
            // slot array and we own this memory exclusively. The caller guarantees proper
            // initialization in the closure.
            let uninit_object = unsafe { object_ptr.cast::<MaybeUninit<T>>().as_mut() };

            // If the closure panics, we just forget this ever happened - we have not modified
            // any slab state yet, so we can back out cleanly.
            f(uninit_object);
        }

        // Create a dropper for the object we just initialized, so we can drop it later.
        //
        // SAFETY: pointer is valid and the caller guarantees that the closure properly initialized
        // the object. We have no logic to drop the object in any other way, so the dropper is clear
        // to act. We only ever create this one dropper for the object, so no double-drop can occur.
        let dropper = unsafe { Dropper::new(object_ptr) };

        // Update the slot metadata to mark it as occupied and store the dropper.
        let previous_meta = mem::replace(slot_meta, SlotMeta::Occupied { _dropper: dropper });

        self.next_free_slot_index = match previous_meta {
            SlotMeta::Vacant {
                next_free_slot_index,
            } => next_free_slot_index,
            SlotMeta::Occupied { .. } => {
                unreachable!(
                    "slot {index} was already occupied in Slab<{}> of capacity {}",
                    type_name::<T>(),
                    self.layout.capacity().get()
                );
            }
        };

        // Increment the count since we successfully inserted an object.
        // Cannot overflow because we would hit capacity limits or virtual memory limits first.
        self.count = self.count.wrapping_add(1);

        SlabHandle::new(index, object_ptr)
    }

    /// Removes an object from the slab, dropping it.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the handle belongs to this slab.
    ///
    /// The caller must ensure that the object is still present in the slab. Slab handles are just
    /// fat pointers, so ownership and object lifetime must be managed manually by the caller.
    pub(crate) unsafe fn remove<T: ?Sized>(&mut self, handle: SlabHandle<T>) {
        // we also verify that the pointer matches, because otherwise one might mix up
        // slot 5 in slab A with slot 5 in slab B.
        #[cfg(debug_assertions)]
        {
            // SAFETY: Forwarding safety requirements from the caller (object must be present in slab).
            let object_ptr_erased = unsafe { self.object_ptr_unchecked::<()>(handle.index()) };

            debug_assert!(
                ptr::addr_eq(object_ptr_erased.as_ptr(), handle.ptr().as_ptr(),),
                "handle pointer does not match object {} pointer in slab",
                type_name::<T>(),
            );
        }

        let next_free_slot_index = self.next_free_slot_index;

        // SAFETY: Forwarding safety requirements from the caller (object must be present in slab).
        let slot_meta = unsafe { self.slot_meta_mut(handle.index()) };

        // The Drop implementation of the existing SlotMeta will automatically call the dropper.
        // We return it to the outer scope so we can defer the drop until after the slab has
        // been updated back into its post-remove state (so if drop panics, the slab remains
        // in an internally consistent state).
        let old_meta = mem::replace(
            slot_meta,
            SlotMeta::Vacant {
                next_free_slot_index,
            },
        );

        // We have a safety requirement that the object must exist in the slab, so this is only
        // an extra check to go above and beyond the call of duty in debug builds, to help detect
        // violations of this safety requirement.
        #[cfg(debug_assertions)]
        if !matches!(old_meta, SlotMeta::Occupied { .. }) {
            // Uh-oh, we were told to remove an object that did not actually exist.
            // While this is a no-no and we will panic, we still need to preserve
            // the pool in a valid state after this (if only for a proper drop to happen).

            // All we did was overwrite the slot with a "vacant" sign,
            // so to restore the previous state we just put back the old meta.
            *slot_meta = old_meta;

            panic!(
                "remove::<{}>() slot {} was vacant in slab of capacity {}",
                type_name::<T>(),
                handle.index(),
                self.layout.capacity().get()
            );
        }

        // Push the released slot into the freelist.
        self.next_free_slot_index = handle.index();

        // Cannot overflow because we asserted above the removed entry was occupied.
        self.count = self.count.wrapping_sub(1);

        // It is now safe to do the drop. If drop() panics, the slab is still in a valid state.
        drop(old_meta);
    }

    /// Removes an object from the slab, returning it.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the handle belongs to this slab.
    ///
    /// The caller must ensure that the object is still present in the slab. Slab handles are just
    /// fat pointers, so ownership and object lifetime must be managed manually by the caller.
    #[must_use]
    pub(crate) unsafe fn remove_unpin<T: Unpin>(&mut self, handle: SlabHandle<T>) -> T {
        const {
            assert!(
                size_of::<T>() > 0,
                "cannot extract zero-sized type from pool"
            );
        };

        // we also verify that the pointer matches, because otherwise one might mix up
        // slot 5 in slab A with slot 5 in slab B.
        #[cfg(debug_assertions)]
        {
            // SAFETY: Forwarding safety requirements from the caller (object must be present in slab).
            let object_ptr_erased = unsafe { self.object_ptr_unchecked::<()>(handle.index()) };

            debug_assert!(
                ptr::addr_eq(object_ptr_erased.as_ptr(), handle.ptr().as_ptr(),),
                "handle pointer does not match object {} pointer in slab",
                type_name::<T>(),
            );
        }

        let next_free_slot_index = self.next_free_slot_index;

        // SAFETY: Forwarding safety requirements from the caller (object must be present in slab).
        let slot_meta = unsafe { self.slot_meta_mut(handle.index()) };

        let old_meta = mem::replace(
            slot_meta,
            SlotMeta::Vacant {
                next_free_slot_index,
            },
        );

        // We have a safety requirement that the object must exist in the slab, so this is only
        // an extra check to go above and beyond the call of duty in debug builds, to help detect
        // violations of this safety requirement.
        #[cfg(debug_assertions)]
        if !matches!(old_meta, SlotMeta::Occupied { .. }) {
            // Uh-oh, we were told to remove an object that did not actually exist.
            // While this is a no-no and we will panic, we still need to preserve
            // the pool in a valid state after this (if only for a proper drop to happen).

            // All we did was overwrite the slot with a "vacant" sign,
            // so to restore the previous state we just put back the old meta.
            *slot_meta = old_meta;

            panic!(
                "remove::<{}>() slot {} was vacant in slab of capacity {}",
                type_name::<T>(),
                handle.index(),
                self.layout.capacity().get()
            );
        }

        // We deliberately do NOT drop the existing slot meta here, instead forgetting it.
        // This prevents the dropper from running, which would drop the object we want to
        // return to the caller.
        mem::forget(old_meta);

        // Read the value from the slot so we can return it to the caller.
        // SAFETY: The caller guarantees the handle points to a valid object of type T in the slab.
        // We have exclusive access through &mut self, and we've verified the slot was occupied.
        let value = unsafe { handle.ptr().read() };

        // Push the released slot into the freelist.
        self.next_free_slot_index = handle.index();

        // Cannot overflow because we asserted above the removed entry was occupied.
        self.count = self.count.wrapping_sub(1);

        value
    }

    /// # Safety
    ///
    /// The caller must ensure that `index` is not out of bounds.
    unsafe fn slot_ptr_unchecked(&self, index: usize) -> NonNull<SlotMeta> {
        // Safety requirement implies we are in bounds, so it overflow because that
        // would imply the slab extends beyond virtual memory, which is not a valid slab layout.
        let offset = index.wrapping_mul(self.layout.slot_layout().size());

        // SAFETY: first_slot_ptr is valid from our allocation in new(), offset is within
        // bounds due to the safety requirements and the wrap logic above, and byte_add
        // preserves pointer validity.
        unsafe { self.first_slot_ptr.byte_add(offset) }
    }

    /// # Safety
    ///
    /// The caller must ensure that `index` is not out of bounds.
    unsafe fn slot_meta_mut(&mut self, index: usize) -> &mut SlotMeta {
        // We define a safety requirement that a handle must actually point to an item
        // in the pool. This assertion is merely an extra safeguard in debug builds, to help
        // detect issues without having to rely on Miri catching the UB. We also do additional
        // validation later on.
        debug_assert!(
            index < self.layout.capacity().get(),
            "slot {index} is out of bounds in slab of capacity {}",
            self.layout.capacity().get()
        );

        // SAFETY: Guarded by above assertion.
        let mut slot_ptr = unsafe { self.slot_ptr_unchecked(index) };

        // SAFETY: slot_ptr was validated by above bounds checking and points to
        // an initialized SlotMeta that we own exclusively (we hold &mut self).
        unsafe { slot_ptr.as_mut() }
    }

    /// # Safety
    ///
    /// The caller must ensure that `index` is not out of bounds.
    unsafe fn object_ptr_unchecked<T>(&self, index: usize) -> NonNull<T> {
        // SAFETY: Forwarding safety guarantees from caller.
        let slot_ptr = unsafe { self.slot_ptr_unchecked(index) };

        // SAFETY: slot_ptr is presumed valid and slot_to_object_offset is guaranteed by
        // SlabLayout to be the offset we need to access the object in the slot.
        unsafe {
            slot_ptr
                .byte_add(self.layout.slot_to_object_offset())
                .cast::<T>()
        }
    }

    /// Returns an iterator over all occupied slots in the slab.
    ///
    /// The iterator yields untyped pointers (`NonNull<()>`) to the objects stored in the slab.
    /// It is the caller's responsibility to cast these pointers to the appropriate type.
    #[must_use]
    pub(crate) fn iter(&self) -> SlabIterator<'_> {
        SlabIterator::new(self)
    }
}

impl Drop for Slab {
    fn drop(&mut self) {
        let was_empty = self.is_empty();
        let original_count = self.count;
        let capacity = self.layout.capacity().get();

        // If a drop() panics, we continue to drop() all the items and release the pool itself,
        // after which we later re-throw the first panic we caught from a drop() invocation.
        let mut first_panic: Option<Box<dyn Any + Send + 'static>> = None;

        // Manually drop all SlotEntry instances to ensure occupied entries are properly dropped.
        // This will automatically call the dropper for any Occupied entries.
        for index in 0..capacity {
            // SAFETY: Guaranteed in-bounds because we are iterating over all our slots.
            let slot_ptr = unsafe { self.slot_ptr_unchecked(index) };

            let drop_result = catch_unwind(AssertUnwindSafe(|| {
                // SAFETY: We allocated and initialized these SlotMeta instances in new(), potentially
                // replacing them in insert()/remove(). This is the final drop any slot undergoes
                // before deallocating the memory.
                unsafe {
                    ptr::drop_in_place(slot_ptr.as_ptr());
                }
            }));

            first_panic = first_panic.or_else(|| drop_result.err());
        }

        // SAFETY: We are using the same layout, provided by the `layout` field.
        // The memory was not yet deallocated.
        unsafe {
            dealloc(
                self.first_slot_ptr.as_ptr().cast(),
                self.layout.slot_array_layout(),
            );
        }

        // If any drop() panicked, re-throw the first panic we caught now that we've cleaned up.
        if let Some(panic) = first_panic {
            resume_unwind(panic);
        }

        // We do this check at the end so we clean up the memory first.
        //
        // If we are already panicking, we do not want to panic again because that will
        // simply obscure whatever the original panic was, leading to debug difficulties.
        if !thread::panicking() && matches!(self.drop_policy, DropPolicy::MustNotDropContents) {
            assert!(
                was_empty,
                "dropped a non-empty slab with {original_count} items - this is forbidden by DropPolicy::MustNotDropContents"
            );
        }
    }
}

/// Iterator over occupied slots in a slab.
///
/// This iterator yields untyped pointers to objects stored in the slab. Since the slab
/// can contain objects of different types (as long as they have the same layout), the
/// iterator returns `NonNull<()>` and leaves type casting to the caller.
///
/// The iterator makes no promises about the pointers being safe to access - the objects pointed
/// to are owned by whoever inserted them and they may hold exclusive references to the objects.
///
/// Supports bidirectional iteration via `DoubleEndedIterator` and exact size tracking
/// via `ExactSizeIterator`.
///
/// # Thread safety
///
/// The type is single-threaded.
#[derive(Debug)]
pub(crate) struct SlabIterator<'s> {
    slab: &'s Slab,

    // Next front iteration will return at this index (if occupied).
    current_front_index: usize,

    // Next back iteration will return at this index minus one (if occupied).
    current_back_index: usize,

    // Used for tracking remaining item count.
    yielded_count: usize,
}

impl<'s> SlabIterator<'s> {
    fn new(slab: &'s Slab) -> Self {
        Self {
            slab,
            current_front_index: 0,
            current_back_index: slab.layout.capacity().get(),
            yielded_count: 0,
        }
    }
}

impl Iterator for SlabIterator<'_> {
    type Item = NonNull<()>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.yielded_count < self.slab.count {
            let entry_index = self.current_front_index;

            // Will not wrap because loop condition stops us
            // when the front and back index meet (at latest).
            self.current_front_index = self.current_front_index.wrapping_add(1);

            // SAFETY: Guaranteed in-bounds since we are iterating over all our slots.
            let slot_ptr = unsafe { self.slab.slot_ptr_unchecked(entry_index) };

            // SAFETY: slot_ptr() guarantees the result is a valid slot.
            let slot_meta = unsafe { slot_ptr.as_ref() };

            if matches!(slot_meta, SlotMeta::Occupied { .. }) {
                // Will not wrap because that would imply we yielded more
                // items than the size of virtual memory, which is impossible.
                self.yielded_count = self.yielded_count.wrapping_add(1);

                // SAFETY: Guaranteed in-bounds since we are iterating over all our slots.
                return Some(unsafe { self.slab.object_ptr_unchecked::<()>(entry_index) });
            }
        }

        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.len();
        (remaining, Some(remaining))
    }
}

impl DoubleEndedIterator for SlabIterator<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        while self.yielded_count < self.slab.count {
            // Will not wrap because loop condition stops us
            // when the front and back index meet (at latest).
            self.current_back_index = self.current_back_index.wrapping_sub(1);

            let entry_index = self.current_back_index;

            // SAFETY: Guaranteed in-bounds since we are iterating over all our slots.
            let slot_ptr = unsafe { self.slab.slot_ptr_unchecked(entry_index) };

            // SAFETY: slot_ptr() guarantees the result is a valid slot.
            let slot_meta = unsafe { slot_ptr.as_ref() };

            if matches!(slot_meta, SlotMeta::Occupied { .. }) {
                // Will not wrap because that would imply we yielded more
                // items than the size of virtual memory, which is impossible.
                self.yielded_count = self.yielded_count.wrapping_add(1);

                // SAFETY: Guaranteed in-bounds since we are iterating over all our slots.
                return Some(unsafe { self.slab.object_ptr_unchecked::<()>(entry_index) });
            }
        }

        None
    }
}

impl ExactSizeIterator for SlabIterator<'_> {
    fn len(&self) -> usize {
        // Total occupied slots in slab minus those we've already yielded
        // Will not wrap because we cannot yield more items than exist in the slab.
        self.slab.count.wrapping_sub(self.yielded_count)
    }
}

// Once we return None, we will keep returning None.
impl FusedIterator for SlabIterator<'_> {}

/// Ensures that the virtual memory pages that make up the slab capacity
/// are mapped to physical memory pages.
///
/// The operating system normally maps virtual pages to physical pages on-demand. However, this
/// has some caveats: it may happen on hot paths and inside critical sections, which is not
/// desirable; furthermore, some operating system APIs (e.g. reading from a Windows socket)
/// seem to switch to a slower logic path if provided virtual pages that are not yet mapped to
/// physical pages. Therefore, it is beneficial to ensure memory is mapped to physical pages
/// ahead of time if it is known to eventually be used.
///
/// We are operating in context of object pooling, so we can assume all its memory will eventually
/// be used, so we do this eagerly whenever we create a new slab, trading off slower initialization
/// for faster object insertion.
///
/// We implement this by writing to each page of memory in the pointer's range, so the memory
/// is assumed to be uninitialized. Calling this on initialized memory would conceptually be
/// pointless, anyway.
#[cfg_attr(test, mutants::skip)] // Impractical to test, as it is merely an optimization that is rarely visible.
fn ensure_virtual_pages_mapped_to_physical_pages(ptr: NonNull<SlotMeta>, layout: &SlabLayout) {
    if layout.slot_layout().size() < 4096 {
        // If the slot is smaller than a typical page, then merely initializing the slot
        // metadata will be enough to touch every page and we can skip this optimization.
        return;
    }

    // We treat the capacity as an array of bytes.
    let ptr = ptr.cast::<u8>();
    let length = layout.slot_array_layout().size();

    // SAFETY: This covers the entire slot memory capacity.
    unsafe {
        ptr.write_bytes(0x3F, length);
    }
}

#[cfg(test)]
mod tests {
    use std::mem::offset_of;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    // Intentionally not Send to avoid accidents. The type API documentation does allow it to be
    // treated as Send if the owner guarantees all contents are Send, but via unsafe code only.
    assert_not_impl_any!(Slab: Send);

    // We do not have any reason to avoid Sync - in principle, the type is only !Sync because the
    // pointers inside disable the auto trait. If we wanted to, we could be Sync. However, we do
    // not want to - simply to preserve design freedom for future implementation changes that may
    // be in conflict with Sync.
    assert_not_impl_any!(Slab: Sync);

    assert_impl_all!(SlabIterator<'_>: Iterator, DoubleEndedIterator, ExactSizeIterator, FusedIterator);
    assert_not_impl_any!(SlabIterator<'_>: Send, Sync);

    /// # Safety
    ///
    /// The caller must guarantee that the layout of `T` matches the object layout of the slab.
    unsafe fn insert<T>(slab: &mut Slab, value: T) -> SlabHandle<T> {
        // SAFETY: Caller guarantees T layout matches slab layout, then we initialize.
        unsafe {
            slab.insert_with_unchecked(|slot| {
                slot.write(value);
            })
        }
    }

    fn smoke_test_impl<T: Default>() {
        // Test basic slab operations with default values of T.
        // 1. We insert 3 items.
        // 2. We remove the second item.
        // 3. We insert 2 more items.
        // 4. We remove the first item.
        // 5. We drop the slab and let it clean up itself (as drop policy == MayDropContents).
        //
        // At relevant points, we check the slab length and emptiness/fullness.
        let layout = SlabLayout::new(Layout::new::<T>());
        let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

        assert!(slab.is_empty());
        assert!(!slab.is_full());
        assert_eq!(slab.len(), 0);

        // 1. Insert 3 items
        // SAFETY: T::default() creates a valid T with matching layout
        let handle1 = unsafe { insert(&mut slab, T::default()) };
        // SAFETY: T::default() creates a valid T with matching layout
        let handle2 = unsafe { insert(&mut slab, T::default()) };
        // SAFETY: T::default() creates a valid T with matching layout
        let handle3 = unsafe { insert(&mut slab, T::default()) };

        assert!(!slab.is_empty());
        assert!(!slab.is_full()); // Slab should still have capacity after 3 insertions
        assert_eq!(slab.len(), 3);

        // 2. Remove the second item
        // SAFETY: handle2 is valid and from this slab
        unsafe {
            slab.remove(handle2);
        }
        assert_eq!(slab.len(), 2);
        assert!(!slab.is_full()); // Still not full after removal

        // 3. Insert 2 more items
        // SAFETY: T::default() creates a valid T with matching layout
        let handle4 = unsafe { insert(&mut slab, T::default()) };
        // SAFETY: T::default() creates a valid T with matching layout
        let handle5 = unsafe { insert(&mut slab, T::default()) };
        assert_eq!(slab.len(), 4);
        assert!(!slab.is_full()); // Should still have capacity

        // 4. Remove the first item
        // SAFETY: handle1 is valid and from this slab
        unsafe {
            slab.remove(handle1);
        }
        assert_eq!(slab.len(), 3);

        // Clean up remaining handles
        // SAFETY: All handles are valid and from this slab
        unsafe {
            slab.remove(handle3);
        }
        // SAFETY: handle4 is valid and from this slab
        unsafe {
            slab.remove(handle4);
        }
        // SAFETY: handle5 is valid and from this slab
        unsafe {
            slab.remove(handle5);
        }
        assert!(slab.is_empty());

        // 5. Slab drops automatically with MayDropContents policy
    }

    #[expect(
        dead_code,
        reason = "Test structs to verify slab operations with complex types"
    )]
    #[test]
    fn smoke_test() {
        struct FunkyStuff {
            a: u32,
            b: MaybeUninit<u64>,
            c: [u8; 123],
            d: Mutex<[u128; 7]>,
        }

        impl Default for FunkyStuff {
            fn default() -> Self {
                Self {
                    a: 42,
                    b: MaybeUninit::uninit(),
                    c: [7; 123],
                    d: Mutex::new([0; 7]),
                }
            }
        }

        struct VeryLarge {
            data: [u8; 64_000],
        }

        impl Default for VeryLarge {
            fn default() -> Self {
                #[expect(clippy::large_stack_arrays, reason = "good enough for test code")]
                Self { data: [1; 64_000] }
            }
        }

        smoke_test_impl::<u8>();
        smoke_test_impl::<u32>();
        smoke_test_impl::<(u32, u64)>();
        smoke_test_impl::<FunkyStuff>();
        smoke_test_impl::<VeryLarge>();
    }

    #[test]
    fn remove_unpin_returns_original_object() {
        let layout = SlabLayout::new(Layout::new::<u32>());
        let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

        let original_value = 42_u32;
        // SAFETY: u32 layout matches slab layout
        let handle = unsafe { insert(&mut slab, original_value) };

        // SAFETY: handle is valid and from this slab
        let returned_value = unsafe { slab.remove_unpin(handle) };
        assert_eq!(returned_value, original_value);
        assert!(slab.is_empty());
    }

    #[test]
    fn handles_medium_sized_objects() {
        // Test with objects larger than typical page size
        struct LargeObject {
            data: [u8; 8192], // 8KB object
        }

        let layout = SlabLayout::new(Layout::new::<LargeObject>());
        let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

        let large_obj = LargeObject { data: [123; 8192] };
        // SAFETY: LargeObject layout matches slab layout
        let handle = unsafe { insert(&mut slab, large_obj) };

        // Verify we can retrieve the object
        // SAFETY: handle is valid and from this slab
        let retrieved = unsafe { slab.remove_unpin(handle) };
        assert_eq!(retrieved.data[0], 123);
        assert_eq!(retrieved.data[8191], 123);
        assert!(slab.is_empty());
    }

    #[test]
    fn remove_with_panicking_drop_keeps_slab_valid() {
        // Object that always panics on drop
        struct PanickingDrop {
            #[allow(dead_code, reason = "Field used to give struct non-zero size")]
            value: u32,
        }

        impl Drop for PanickingDrop {
            fn drop(&mut self) {
                panic!("intentional panic in drop");
            }
        }

        let layout = SlabLayout::new(Layout::new::<PanickingDrop>());
        let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

        // Insert a panicking object
        // SAFETY: PanickingDrop layout matches slab layout
        let panicking_handle = unsafe { insert(&mut slab, PanickingDrop { value: 42 }) };
        assert_eq!(slab.len(), 1);
        assert!(!slab.is_empty());

        let original_index = panicking_handle.index();

        // Remove the object - this should panic during drop but still remove the object
        let panic_result = catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: panicking_handle is valid and from this slab
            unsafe {
                slab.remove(panicking_handle);
            }
        }));

        // The removal should have panicked
        assert!(panic_result.is_err());

        // But the slab should still be in a valid state:
        // 1. The object should be removed (count should be 0)
        assert_eq!(slab.len(), 0);
        assert!(slab.is_empty());

        // 2. The slot should be available for reuse (inserting should use the same index)
        // SAFETY: u32 layout matches slab layout
        let new_handle = unsafe { insert(&mut slab, 100_u32) };
        assert_eq!(new_handle.index(), original_index);
        assert_eq!(slab.len(), 1);

        // Clean up
        // SAFETY: new_handle is valid and from this slab
        unsafe {
            slab.remove(new_handle);
        }
    }

    #[test]
    #[should_panic]
    fn drop_policy_must_not_drop_items_causes_panic_on_drop_with_items() {
        let layout = SlabLayout::new(Layout::new::<u32>());
        let mut slab = Slab::new(layout, DropPolicy::MustNotDropContents);

        // SAFETY: u32 layout matches slab layout
        let _handle = unsafe { insert(&mut slab, 42_u32) };

        // Slab drops here with items still present - should panic
    }

    #[test]
    fn insert_with_partial_object_initialize() {
        struct PartiallyInitializable {
            a: u32,
            #[allow(dead_code, reason = "Field intentionally not initialized in test")]
            b: MaybeUninit<u64>,
        }

        impl PartiallyInitializable {
            fn new_in_place(place: &mut MaybeUninit<Self>) {
                // SAFETY: Compiler-guaranteed offsetting of pointer to field within struct.
                let a_ptr = unsafe {
                    place
                        .as_mut_ptr()
                        .cast::<u32>()
                        .byte_add(offset_of!(Self, a))
                };

                // SAFETY: We got the pointer from an exclusive reference, so all must be well.
                unsafe {
                    a_ptr.write(42);
                }

                // We deliberately do NOT initialize `b`.
            }
        }

        let layout = SlabLayout::new(Layout::new::<PartiallyInitializable>());
        let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

        // SAFETY: PartiallyInitializable layout matches slab layout,
        // and new_in_place properly initializes required fields
        let handle = unsafe {
            slab.insert_with_unchecked(|uninit| {
                PartiallyInitializable::new_in_place(uninit);
            })
        };

        // Verify the object was inserted
        assert_eq!(slab.len(), 1);

        // Clean up
        // SAFETY: handle is valid and from this slab
        unsafe {
            slab.remove(handle);
        }
    }

    #[test]
    #[should_panic]
    #[cfg(debug_assertions)]
    fn debug_build_double_remove_panics() {
        let layout = SlabLayout::new(Layout::new::<u32>());
        let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

        // SAFETY: u32 layout matches slab layout
        let handle = unsafe { insert(&mut slab, 42_u32) };

        // SAFETY: handle is valid and from this slab
        unsafe {
            slab.remove(handle);
        }
        // SAFETY: This should panic - handle is no longer valid
        unsafe {
            slab.remove(handle);
        } // Should panic
    }

    #[test]
    #[should_panic]
    #[cfg(debug_assertions)]
    fn debug_build_double_remove_unpin_panics() {
        let layout = SlabLayout::new(Layout::new::<u32>());
        let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

        // SAFETY: u32 layout matches slab layout
        let handle = unsafe { insert(&mut slab, 42_u32) };

        // SAFETY: handle is valid and from this slab
        unsafe {
            _ = slab.remove_unpin(handle);
        }
        // SAFETY: This should panic - handle is no longer valid
        unsafe {
            _ = slab.remove_unpin(handle);
        } // Should panic
    }

    #[test]
    #[should_panic]
    #[cfg(debug_assertions)]
    fn debug_build_remove_with_handle_from_wrong_slab_panics() {
        let layout = SlabLayout::new(Layout::new::<u32>());
        let mut slab1 = Slab::new(layout, DropPolicy::MayDropContents);
        let mut slab2 = Slab::new(layout, DropPolicy::MayDropContents);

        // SAFETY: u32 layout matches slab layout
        let _handle_from_slab1 = unsafe { insert(&mut slab1, 42_u32) };
        // SAFETY: u32 layout matches slab layout
        let handle_from_slab2 = unsafe { insert(&mut slab2, 42_u32) };

        // Try to remove handle from slab2 using slab1 - should panic
        // SAFETY: This should panic - handle is from wrong slab
        unsafe {
            slab1.remove(handle_from_slab2);
        }
    }

    #[test]
    #[should_panic]
    #[cfg(debug_assertions)]
    fn debug_build_remove_unpin_with_handle_from_wrong_slab_panics() {
        let layout = SlabLayout::new(Layout::new::<u32>());
        let mut slab1 = Slab::new(layout, DropPolicy::MayDropContents);
        let mut slab2 = Slab::new(layout, DropPolicy::MayDropContents);

        // SAFETY: u32 layout matches slab layout
        let _handle_from_slab1 = unsafe { insert(&mut slab1, 42_u32) };
        // SAFETY: u32 layout matches slab layout
        let handle_from_slab2 = unsafe { insert(&mut slab2, 42_u32) };

        // Try to remove handle from slab2 using slab1 - should panic
        // SAFETY: This should panic - handle is from wrong slab
        unsafe {
            _ = slab1.remove_unpin(handle_from_slab2);
        }
    }

    #[test]
    #[should_panic]
    // Miri correctly detects the OOB pointer arithmetic, so "fails" the test.
    // This behavior is acceptable because being in-bounds is anyway a safety requirement.
    // We are just being extra thorough here by explicitly checking, which is potentially overkill.
    #[cfg_attr(miri, ignore)]
    fn debug_build_remove_with_out_of_bounds_handle_from_wrong_slab_panics() {
        // We create slabs with different capacities - a slab with large items, meaning it
        // has a small capacity, and a slab with small items, meaning it has a large capacity.
        // Trying to remove from the large object slab using the handle from the small object
        // slab will indicate an index out of bounds of the large object slab.

        let layout1 = SlabLayout::new(Layout::new::<[u8; 64_000]>());
        let layout2 = SlabLayout::new(Layout::new::<u32>());
        let mut slab1 = Slab::new(layout1, DropPolicy::MayDropContents);
        let mut slab2 = Slab::new(layout2, DropPolicy::MayDropContents);

        // SAFETY: layout matches slab layout
        #[expect(clippy::large_stack_arrays, reason = "good enough for test code")]
        let _handle_from_slab1 = unsafe { insert(&mut slab1, [0_u8; 64_000]) };

        let handle_from_slab2 = {
            let mut last_handle = None;

            for _ in 0..layout2.capacity().get() {
                // SAFETY: u32 layout matches slab layout
                let handle = unsafe { insert(&mut slab2, 42_u32) };
                last_handle = Some(handle);
            }

            last_handle.unwrap()
        };

        // Try to remove handle from slab2 using slab1 - should panic
        // SAFETY: We are intentionally violating the safety requirements.
        unsafe {
            slab1.remove(handle_from_slab2);
        }
    }

    #[test]
    fn drops_inserted_object_on_slab_drop() {
        static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

        struct DropCounter {
            #[allow(dead_code, reason = "Field used to give struct non-zero size")]
            value: u8,
        }

        impl Drop for DropCounter {
            fn drop(&mut self) {
                DROP_COUNT.fetch_add(1, Ordering::SeqCst);
            }
        }

        DROP_COUNT.store(0, Ordering::SeqCst);

        {
            let layout = SlabLayout::new(Layout::new::<DropCounter>());
            let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

            // SAFETY: DropCounter layout matches slab layout
            let _handle1 = unsafe { insert(&mut slab, DropCounter { value: 1 }) };
            // SAFETY: DropCounter layout matches slab layout
            let _handle2 = unsafe { insert(&mut slab, DropCounter { value: 2 }) };

            // Slab drops here, should drop both objects
        }

        assert_eq!(DROP_COUNT.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn mixed_objects_with_same_layout() {
        // Test that different types with the same layout can be stored in the same slab
        // and that basic operations work correctly for both types.

        // Two different types with identical layouts (both are 4-byte aligned u32)
        #[repr(transparent)]
        struct TypeA(u32);

        #[repr(transparent)]
        struct TypeB(u32);

        // Verify they have the same layout
        assert_eq!(Layout::new::<TypeA>(), Layout::new::<TypeB>());

        let layout = SlabLayout::new(Layout::new::<TypeA>());
        let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

        // Insert objects of different types
        // SAFETY: TypeA layout matches slab layout
        let handle_a1 = unsafe { insert(&mut slab, TypeA(100)) };
        // SAFETY: TypeB has same layout as TypeA, so it matches slab layout
        let handle_b1 = unsafe { insert(&mut slab, TypeB(200)) };
        // SAFETY: TypeA layout matches slab layout
        let handle_a2 = unsafe { insert(&mut slab, TypeA(300)) };
        // SAFETY: TypeB has same layout as TypeA, so it matches slab layout
        let handle_b2 = unsafe { insert(&mut slab, TypeB(400)) };

        assert_eq!(slab.len(), 4);
        assert!(!slab.is_empty());

        // Remove objects in mixed order
        // SAFETY: handle_b1 is valid and from this slab
        unsafe {
            slab.remove(handle_b1);
        }
        assert_eq!(slab.len(), 3);

        // SAFETY: handle_a1 is valid and from this slab
        unsafe {
            slab.remove(handle_a1);
        }
        assert_eq!(slab.len(), 2);

        // Test remove_unpin with mixed types
        // SAFETY: handle_b2 is valid and from this slab
        let returned_b = unsafe { slab.remove_unpin(handle_b2) };
        assert_eq!(returned_b.0, 400);
        assert_eq!(slab.len(), 1);

        // SAFETY: handle_a2 is valid and from this slab
        let returned_a = unsafe { slab.remove_unpin(handle_a2) };
        assert_eq!(returned_a.0, 300);
        assert!(slab.is_empty());

        // Insert more objects after clearing
        // SAFETY: TypeB has same layout as TypeA, so it matches slab layout
        let handle_b3 = unsafe { insert(&mut slab, TypeB(500)) };
        // SAFETY: TypeA layout matches slab layout
        let handle_a3 = unsafe { insert(&mut slab, TypeA(600)) };

        assert_eq!(slab.len(), 2);

        // Clean up
        // SAFETY: handle_b3 is valid and from this slab
        unsafe {
            slab.remove(handle_b3);
        }
        // SAFETY: handle_a3 is valid and from this slab
        unsafe {
            slab.remove(handle_a3);
        }

        assert!(slab.is_empty());
    }

    #[test]
    #[should_panic]
    fn debug_build_insert_panics_when_slab_is_full() {
        // Create a slab with minimal capacity to make it easy to fill
        let layout = SlabLayout::new(Layout::new::<u32>());
        let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

        let capacity = layout.capacity().get();

        // Fill the slab to capacity
        for i in 0..capacity {
            // SAFETY: u32 layout matches slab layout
            #[allow(
                clippy::cast_possible_truncation,
                reason = "test uses small capacity values"
            )]
            let _handle = unsafe { insert(&mut slab, i as u32) };
        }

        assert!(slab.is_full());
        assert_eq!(slab.len(), capacity);

        // This should panic - slab is full.
        // NB! This only panics if debug_assertions is enabled. This is not part of the API
        // contract, rather it is a debug build sanity check.
        // SAFETY: u32 layout matches slab layout, but slab is full so should panic
        unsafe {
            insert(&mut slab, 999_u32);
        }
    }

    #[test]
    #[should_panic]
    fn debug_build_wrong_layout_panics() {
        let layout = SlabLayout::new(Layout::new::<u32>());
        let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

        // This should panic - layout of u64 does not match slab layout of u32.
        // NB! This only panics if debug_assertions is enabled. This is not part of the API
        // contract, rather it is a debug build sanity check.
        // SAFETY: intentionally violating safety requirements here.
        unsafe {
            insert(&mut slab, 123_u64);
        }
    }

    #[test]
    fn is_full_comprehensive_behavior() {
        // Create a slab with minimal capacity to test full behavior
        let layout = SlabLayout::new(Layout::new::<usize>());
        let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

        let capacity = layout.capacity().get();

        // Initially slab should not be full
        assert!(!slab.is_full());
        assert!(slab.is_empty());
        assert_eq!(slab.len(), 0);

        let mut handles = Vec::new();

        // Fill the slab one item at a time, verifying is_full() at each step
        for i in 0..capacity {
            // Should not be full before inserting (except when about to reach capacity)
            assert!(!slab.is_full());

            // SAFETY: usize layout matches slab layout
            let handle = unsafe { insert(&mut slab, i) };
            handles.push(handle);

            assert_eq!(slab.len(), i + 1);
            assert!(!slab.is_empty());

            // Check if we're now full
            if i + 1 == capacity {
                // Should be full now
                assert!(slab.is_full());
                assert_eq!(slab.len(), capacity);
            } else {
                // Should not be full yet
                assert!(!slab.is_full());
            }
        }

        // Slab should definitely be full now
        assert!(slab.is_full());
        assert_eq!(slab.len(), capacity);
        assert!(!slab.is_empty());

        // Remove one item - should no longer be full
        if !handles.is_empty() {
            let handle_to_remove = handles.pop().unwrap();
            // SAFETY: handle is valid and from this slab
            unsafe {
                slab.remove(handle_to_remove);
            }

            // Should no longer be full
            assert!(!slab.is_full());
            assert_eq!(slab.len(), capacity - 1);
            assert!(!slab.is_empty());
        }

        // Clean up remaining handles
        for handle in handles {
            // SAFETY: All handles are valid and from this slab
            unsafe {
                slab.remove(handle);
            }
        }

        // Should be empty again
        assert!(!slab.is_full());
        assert!(slab.is_empty());
        assert_eq!(slab.len(), 0);
    }

    #[test]
    fn insertion_follows_lowest_index_first_order() {
        // This is not a promise in the API contract - we are allowed to insert in any order
        // we want, as the indexes are an internal implementation detail of the slabs. However,
        // our current implementation fills from the lowest index first, so we have a test here
        // to verify this behavior remains stable until we choose to change it.

        let layout = SlabLayout::new(Layout::new::<u32>());
        let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

        assert!(!slab.is_full());
        assert!(slab.is_empty());

        // Insert items and verify they get indices 0, 1, 2
        // SAFETY: u32 layout matches slab layout
        let handle0 = unsafe { insert(&mut slab, 100_u32) };
        assert!(!slab.is_full());
        assert!(!slab.is_empty());

        // SAFETY: u32 layout matches slab layout
        let handle1 = unsafe { insert(&mut slab, 200_u32) };
        assert!(!slab.is_full());

        // SAFETY: u32 layout matches slab layout
        let handle2 = unsafe { insert(&mut slab, 300_u32) };

        assert_eq!(handle0.index(), 0);
        assert_eq!(handle1.index(), 1);
        assert_eq!(handle2.index(), 2);
        assert_eq!(slab.len(), 3);
        assert!(!slab.is_full()); // Slab should still have capacity

        // Remove the middle item (index 1)
        // SAFETY: handle1 is valid and from this slab
        unsafe {
            slab.remove(handle1);
        }
        assert_eq!(slab.len(), 2);

        // Next insertion should reuse the freed slot at index 1
        // SAFETY: u32 layout matches slab layout
        let handle_reuse = unsafe { insert(&mut slab, 400_u32) };
        assert_eq!(handle_reuse.index(), 1);
        assert_eq!(slab.len(), 3);

        // Next insertion should use index 3 (next available)
        // SAFETY: u32 layout matches slab layout
        let handle3 = unsafe { insert(&mut slab, 500_u32) };
        assert_eq!(handle3.index(), 3);
        assert_eq!(slab.len(), 4);

        // Remove index 0 and 2, leaving 1 and 3 occupied
        // SAFETY: handle0 is valid and from this slab
        unsafe {
            slab.remove(handle0);
        }
        // SAFETY: handle2 is valid and from this slab
        unsafe {
            slab.remove(handle2);
        }
        assert_eq!(slab.len(), 2);

        // Next insertion should reuse index 2 (most recently freed, stack behavior)
        // SAFETY: u32 layout matches slab layout
        let handle_reuse2 = unsafe { insert(&mut slab, 600_u32) };
        assert_eq!(handle_reuse2.index(), 2);

        // Then index 0 should be reused
        // SAFETY: u32 layout matches slab layout
        let handle_reuse0 = unsafe { insert(&mut slab, 700_u32) };
        assert_eq!(handle_reuse0.index(), 0);
    }

    #[test]
    fn iter_empty_slab() {
        let layout = SlabLayout::new(Layout::new::<u32>());
        let slab = Slab::new(layout, DropPolicy::MayDropContents);

        let mut iter = slab.iter();
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert_eq!(iter.len(), 0);

        assert_eq!(iter.next(), None);
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert_eq!(iter.len(), 0);
    }

    #[test]
    fn iter_single_item() {
        let layout = SlabLayout::new(Layout::new::<u32>());
        let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

        // SAFETY: u32 layout matches slab layout
        let _handle = unsafe { insert(&mut slab, 42_u32) };

        let mut iter = slab.iter();

        // First item should be the object we inserted
        let ptr = iter.next().expect("should have one item");

        // SAFETY: We know this points to a u32 we just inserted
        let value = unsafe { ptr.cast::<u32>().as_ref() };
        assert_eq!(*value, 42);

        // No more items
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn iter_multiple_items() {
        let layout = SlabLayout::new(Layout::new::<u32>());
        let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

        // Insert multiple items
        // SAFETY: u32 layout matches slab layout
        let _handle1 = unsafe { insert(&mut slab, 100_u32) };
        // SAFETY: u32 layout matches slab layout
        let _handle2 = unsafe { insert(&mut slab, 200_u32) };
        // SAFETY: u32 layout matches slab layout
        let _handle3 = unsafe { insert(&mut slab, 300_u32) };

        let values: Vec<u32> = slab
            .iter()
            .map(|ptr| {
                // SAFETY: We know these point to u32 values we inserted
                unsafe { *ptr.cast::<u32>().as_ref() }
            })
            .collect();

        // Should get all values in order of their slot indices (0, 1, 2)
        assert_eq!(values, vec![100, 200, 300]);
    }

    #[test]
    fn iter_with_gaps() {
        let layout = SlabLayout::new(Layout::new::<u32>());
        let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

        // Insert items
        // SAFETY: u32 layout matches slab layout
        let handle1 = unsafe { insert(&mut slab, 100_u32) };
        // SAFETY: u32 layout matches slab layout
        let handle2 = unsafe { insert(&mut slab, 200_u32) };
        // SAFETY: u32 layout matches slab layout
        let handle3 = unsafe { insert(&mut slab, 300_u32) };

        // Remove the middle item to create a gap
        // SAFETY: handle2 is valid and from this slab
        unsafe {
            slab.remove(handle2);
        }

        // Silence unused variable warnings for handles we don't use after removal
        let _ = (handle1, handle3);

        let values: Vec<u32> = slab
            .iter()
            .map(|ptr| {
                // SAFETY: We know these point to u32 values we inserted
                unsafe { *ptr.cast::<u32>().as_ref() }
            })
            .collect();

        // Should get only the remaining values (slot 1 is vacant)
        assert_eq!(values, vec![100, 300]);
    }

    #[test]
    fn iter_mixed_types_same_layout() {
        // Test iterator with different types that have the same layout
        #[repr(transparent)]
        struct TypeA(u32);

        #[repr(transparent)]
        struct TypeB(u32);

        let layout = SlabLayout::new(Layout::new::<TypeA>());
        let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

        // Insert different types with same layout
        // SAFETY: TypeA layout matches slab layout
        let _handle_a = unsafe { insert(&mut slab, TypeA(100)) };
        // SAFETY: TypeB has same layout as TypeA
        let _handle_b = unsafe { insert(&mut slab, TypeB(200)) };

        let mut values = Vec::new();
        for (i, ptr) in slab.iter().enumerate() {
            if i == 0 {
                // First slot contains TypeA
                // SAFETY: We know this is TypeA(100)
                let value = unsafe { ptr.cast::<TypeA>().as_ref() };
                values.push(value.0);
            } else {
                // Second slot contains TypeB
                // SAFETY: We know this is TypeB(200)
                let value = unsafe { ptr.cast::<TypeB>().as_ref() };
                values.push(value.0);
            }
        }

        assert_eq!(values, vec![100, 200]);
    }

    #[test]
    fn iter_size_hint() {
        let layout = SlabLayout::new(Layout::new::<u32>());
        let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

        // Empty slab
        let iter = slab.iter();
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert_eq!(iter.len(), 0);

        // Add some items
        // SAFETY: u32 layout matches slab layout
        let _handle1 = unsafe { insert(&mut slab, 100_u32) };
        // SAFETY: u32 layout matches slab layout
        let _handle2 = unsafe { insert(&mut slab, 200_u32) };

        let mut iter = slab.iter();
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
        let layout = SlabLayout::new(Layout::new::<u32>());
        let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

        // Insert items at indices 0, 1, 2
        // SAFETY: u32 layout matches slab layout
        let _handle1 = unsafe { insert(&mut slab, 100_u32) };
        // SAFETY: u32 layout matches slab layout
        let _handle2 = unsafe { insert(&mut slab, 200_u32) };
        // SAFETY: u32 layout matches slab layout
        let _handle3 = unsafe { insert(&mut slab, 300_u32) };

        let mut iter = slab.iter();

        // Iterate from the back
        let last_ptr = iter.next_back().expect("should have last item");
        // SAFETY: We know this points to a u32 we inserted
        let last_value = unsafe { *last_ptr.cast::<u32>().as_ref() };
        assert_eq!(last_value, 300);

        let middle_ptr = iter.next_back().expect("should have middle item");
        // SAFETY: We know this points to a u32 we inserted
        let middle_value = unsafe { *middle_ptr.cast::<u32>().as_ref() };
        assert_eq!(middle_value, 200);

        let first_ptr = iter.next_back().expect("should have first item");
        // SAFETY: We know this points to a u32 we inserted
        let first_value = unsafe { *first_ptr.cast::<u32>().as_ref() };
        assert_eq!(first_value, 100);

        // Should be exhausted
        assert_eq!(iter.next_back(), None);
        assert_eq!(iter.next(), None);
    }

    #[test]
    fn iter_double_ended_mixed_directions() {
        let layout = SlabLayout::new(Layout::new::<u32>());
        let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

        // Insert 5 items
        // SAFETY: u32 layout matches slab layout
        let _handle1 = unsafe { insert(&mut slab, 100_u32) };
        // SAFETY: u32 layout matches slab layout
        let _handle2 = unsafe { insert(&mut slab, 200_u32) };
        // SAFETY: u32 layout matches slab layout
        let _handle3 = unsafe { insert(&mut slab, 300_u32) };
        // SAFETY: u32 layout matches slab layout
        let _handle4 = unsafe { insert(&mut slab, 400_u32) };
        // SAFETY: u32 layout matches slab layout
        let _handle5 = unsafe { insert(&mut slab, 500_u32) };

        let mut iter = slab.iter();
        assert_eq!(iter.len(), 5);

        // Get first from front
        let first_ptr = iter.next().expect("should have first item");
        // SAFETY: We know this points to a u32 we inserted
        let first_value = unsafe { *first_ptr.cast::<u32>().as_ref() };
        assert_eq!(first_value, 100);
        assert_eq!(iter.len(), 4);

        // Get last from back
        let last_ptr = iter.next_back().expect("should have last item");
        // SAFETY: We know this points to a u32 we inserted
        let last_value = unsafe { *last_ptr.cast::<u32>().as_ref() };
        assert_eq!(last_value, 500);
        assert_eq!(iter.len(), 3);

        // Get second from front
        let second_ptr = iter.next().expect("should have second item");
        // SAFETY: We know this points to a u32 we inserted
        let second_value = unsafe { *second_ptr.cast::<u32>().as_ref() };
        assert_eq!(second_value, 200);
        assert_eq!(iter.len(), 2);

        // Get fourth from back
        let fourth_ptr = iter.next_back().expect("should have fourth item");
        // SAFETY: We know this points to a u32 we inserted
        let fourth_value = unsafe { *fourth_ptr.cast::<u32>().as_ref() };
        assert_eq!(fourth_value, 400);
        assert_eq!(iter.len(), 1);

        // Get middle item
        let middle_ptr = iter.next().expect("should have middle item");
        // SAFETY: We know this points to a u32 we inserted
        let middle_value = unsafe { *middle_ptr.cast::<u32>().as_ref() };
        assert_eq!(middle_value, 300);
        assert_eq!(iter.len(), 0);

        // Should be exhausted
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next_back(), None);
        assert_eq!(iter.len(), 0);
    }

    #[test]
    fn iter_double_ended_with_gaps() {
        let layout = SlabLayout::new(Layout::new::<u32>());
        let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

        // Insert items and create gaps
        // SAFETY: u32 layout matches slab layout
        let handle1 = unsafe { insert(&mut slab, 100_u32) };
        // SAFETY: u32 layout matches slab layout
        let handle2 = unsafe { insert(&mut slab, 200_u32) };
        // SAFETY: u32 layout matches slab layout
        let handle3 = unsafe { insert(&mut slab, 300_u32) };
        // SAFETY: u32 layout matches slab layout
        let handle4 = unsafe { insert(&mut slab, 400_u32) };
        // SAFETY: u32 layout matches slab layout
        let handle5 = unsafe { insert(&mut slab, 500_u32) };

        // Remove items to create gaps (remove indices 1 and 3)
        // SAFETY: handle2 is valid and from this slab
        unsafe {
            slab.remove(handle2); // Remove 200 (index 1)
        }
        // SAFETY: handle4 is valid and from this slab
        unsafe {
            slab.remove(handle4); // Remove 400 (index 3)
        }

        // Silence unused variable warnings for handles we don't use after removal
        let _ = (handle1, handle3, handle5);

        // Now we have items at indices 0, 2, 4 with values 100, 300, 500
        let mut iter = slab.iter();
        assert_eq!(iter.len(), 3);

        // Get from back first (should be 500 at index 4)
        let back_ptr = iter.next_back().expect("should have back item");
        // SAFETY: We know this points to a u32 we inserted
        let back_value = unsafe { *back_ptr.cast::<u32>().as_ref() };
        assert_eq!(back_value, 500);
        assert_eq!(iter.len(), 2);

        // Get from front (should be 100 at index 0)
        let front_ptr = iter.next().expect("should have front item");
        // SAFETY: We know this points to a u32 we inserted
        let front_value = unsafe { *front_ptr.cast::<u32>().as_ref() };
        assert_eq!(front_value, 100);
        assert_eq!(iter.len(), 1);

        // Get remaining (should be 300 at index 2)
        let remaining_ptr = iter.next().expect("should have remaining item");
        // SAFETY: We know this points to a u32 we inserted
        let remaining_value = unsafe { *remaining_ptr.cast::<u32>().as_ref() };
        assert_eq!(remaining_value, 300);
        assert_eq!(iter.len(), 0);

        // Should be exhausted
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next_back(), None);
    }

    #[test]
    fn iter_fused_behavior() {
        let layout = SlabLayout::new(Layout::new::<u32>());
        let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

        // Test with empty slab
        let mut iter = slab.iter();
        assert_eq!(iter.next(), None);
        assert_eq!(iter.next(), None); // Should still be None
        assert_eq!(iter.next_back(), None);
        assert_eq!(iter.next_back(), None); // Should still be None

        // Test with some items
        // SAFETY: u32 layout matches slab layout
        let _handle1 = unsafe { insert(&mut slab, 100_u32) };
        // SAFETY: u32 layout matches slab layout
        let _handle2 = unsafe { insert(&mut slab, 200_u32) };

        let mut iter = slab.iter();

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
        let mut iter = slab.iter();

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
    fn slab_drop_calls_all_object_drops_even_when_they_panic() {
        static DROP_COUNT: AtomicUsize = AtomicUsize::new(0);

        // Object that always panics on drop but still increments the counter
        struct AlwaysPanickingDrop {
            #[allow(dead_code, reason = "Field used to give struct non-zero size")]
            id: u32,
        }

        impl Drop for AlwaysPanickingDrop {
            fn drop(&mut self) {
                // Increment counter first, then panic
                DROP_COUNT.fetch_add(1, Ordering::SeqCst);
                panic!("intentional panic from drop of object {}", self.id);
            }
        }

        DROP_COUNT.store(0, Ordering::SeqCst);

        let drop_result = catch_unwind(AssertUnwindSafe(|| {
            let layout = SlabLayout::new(Layout::new::<AlwaysPanickingDrop>());
            let mut slab = Slab::new(layout, DropPolicy::MayDropContents);

            // Insert multiple panicking objects
            // SAFETY: AlwaysPanickingDrop layout matches slab layout
            let _handle1 = unsafe { insert(&mut slab, AlwaysPanickingDrop { id: 1 }) };
            // SAFETY: AlwaysPanickingDrop layout matches slab layout
            let _handle2 = unsafe { insert(&mut slab, AlwaysPanickingDrop { id: 2 }) };
            // SAFETY: AlwaysPanickingDrop layout matches slab layout
            let _handle3 = unsafe { insert(&mut slab, AlwaysPanickingDrop { id: 3 }) };
            // SAFETY: AlwaysPanickingDrop layout matches slab layout
            let _handle4 = unsafe { insert(&mut slab, AlwaysPanickingDrop { id: 4 }) };

            assert_eq!(slab.len(), 4);

            // Slab drops here - should call drop() on all 4 objects even though they all panic
        }));

        // The slab drop should have panicked due to the panicking object drops
        assert!(drop_result.is_err());

        // But all 4 objects should have had their drop() called
        assert_eq!(DROP_COUNT.load(Ordering::SeqCst), 4);
    }
}
