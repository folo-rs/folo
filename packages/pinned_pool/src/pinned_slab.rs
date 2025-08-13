use core::panic;
use std::alloc::{Layout, alloc, dealloc};
use std::any::type_name;
use std::mem::MaybeUninit;
use std::pin::Pin;
use std::ptr::NonNull;
use std::{mem, thread};

use crate::DropPolicy;

/// This is the backing storage of a `PinnedPool`. It is currently an implementation detail,
/// though could conceivably be made public in the future to support fixed-size pool needs.
///
/// A pinned fixed-capacity heap-allocated collection. Works similar to a `Vec` but all items are
/// pinned and the collection has a fixed capacity of `CAPACITY` items, operating using an index
/// for lookup. When you insert an item, you can get back the index to use for accessing or
/// removing the item.
///
/// There are multiple ways to insert items into the collection:
///
/// * [`insert()`][3] - inserts a value and returns the index. This is the simplest way to add an
///   item but requires you to later look it up by the index. That lookup is fast but not free.
/// * [`begin_insert().insert()`][4] - returns a shared reference to the inserted item; you may
///   also obtain the index in advance from the inserter through [`index()`][7] which may be
///   useful if the item needs to know its own index in the collection.
/// * [`begin_insert().insert_mut()`][5] - returns an exclusive reference to the inserted item; you
///   may also obtain the index in advance from the inserter through [`index()`][7] which may be
///   useful if the item needs to know its own index in the collection.
/// * [`begin_insert().insert_with()`][8] - allows the caller to initialize the item in-place using
///   a closure that receives a `&mut MaybeUninit<T>`. Returns a shared reference to the item.
/// * [`begin_insert().insert_with_mut()`][9] - allows the caller to initialize the item in-place
///   using a closure that receives a `&mut MaybeUninit<T>`. Returns an exclusive reference to the item.
///
/// # Out of band access
///
/// The collection does not keep references to the items or create new references unless you
/// explicitly ask for one, so it is valid to access items via pointers and to create custom
/// references (including exclusive references) to items from unsafe code even when not holding
/// an exclusive reference to the collection, as long as you do not ask the collection to
/// concurrently create a conflicting reference (e.g. via [`get()`][1] or [`get_mut()`][2]).
///
/// You can obtain pointers to the items via the `Pin<&T>` or `Pin<&mut T>` returned by the
/// [`get()`][1] and [`get_mut()`][2] methods, respectively. These pointers are guaranteed to
/// be valid until the item is removed from the collection or the collection itself is dropped.
///
/// [1]: Self::get
/// [2]: Self::get_mut
/// [3]: Self::insert
/// [4]: PinnedSlabInserter::insert
/// [5]: PinnedSlabInserter::insert_mut
/// [7]: PinnedSlabInserter::index
/// [8]: PinnedSlabInserter::insert_with
/// [9]: PinnedSlabInserter::insert_with_mut
#[derive(Debug)]
pub(crate) struct PinnedSlab<T, const CAPACITY: usize> {
    first_entry_ptr: NonNull<Entry<T>>,

    /// Index of the next free slot in the collection. Think of this as a virtual stack of the most
    /// recently freed slots, with the stack entries stored in the collection entries themselves.
    /// Also known as intrusive freelist. This will point out of bounds if the collection is full.
    next_free_index: usize,

    /// The total number of items in the collection. This is not used by the collection itself but
    /// may be valuable to callers who want to know if the collection is empty because in many use
    /// cases the collection is the backing store for a custom allocation/pinning scheme for items
    /// used from unsafe code and may not be valid to drop when any items are still present.
    count: usize,

    drop_policy: DropPolicy,
}

#[derive(Debug)]
enum Entry<T> {
    Occupied { value: MaybeUninit<T> },

    Vacant { next_free_index: usize },
}

impl<T, const CAPACITY: usize> PinnedSlab<T, CAPACITY> {
    /// Creates a new slab with the specified drop policy.
    ///
    /// # Panics
    ///
    /// Panics if the slab would be zero-sized either due to capacity or item size being zero.
    #[must_use]
    pub(crate) fn new(drop_policy: DropPolicy) -> Self {
        assert!(CAPACITY > 0, "PinnedSlab must have non-zero capacity",);
        assert!(
            size_of::<T>() > 0,
            "PinnedSlab must have non-zero item size"
        );
        assert!(
            CAPACITY < usize::MAX,
            "PinnedSlab capacity must be less than usize::MAX"
        );

        // SAFETY: The layout must be valid for the target type (sure, we calculate it correctly)
        // and not zero-sized (guarded by assertion above).
        let ptr = NonNull::new(unsafe { alloc(Self::layout()).cast::<Entry<T>>() }).expect(
            "we do not intend to handle allocation failure as a real possibility - OOM is panic",
        );

        ensure_virtual_pages_mapped_to_physical_pages::<Entry<T>, CAPACITY>(ptr);

        // Initialize all slots to `Vacant` to start with.
        for index in 0..CAPACITY {
            // SAFETY: We ensure in `layout()` that there is enough space for all items up to our
            // indicated capacity.
            let entry = unsafe { ptr.add(index) };

            // SAFETY: The pointer is valid for writes and of the right type, so all is well.
            unsafe {
                entry.as_ptr().write(Entry::Vacant {
                    // For the last item, this will point out of bounds, which is fine.
                    // It means the slab is full and no more items can be inserted.
                    next_free_index: index
                        .checked_add(1)
                        .expect("guarded by capacity < usize::MAX above"),
                });
            }
        }

        Self {
            first_entry_ptr: ptr,
            next_free_index: 0,
            count: 0,
            drop_policy,
        }
    }

    #[must_use]
    fn layout() -> Layout {
        Layout::array::<Entry<T>>(CAPACITY).expect("simple flat array layout must be calculable")
    }

    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Can be mutated to infinitely growing memory use.
    pub(crate) fn len(&self) -> usize {
        self.count
    }

    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.count == 0
    }

    #[must_use]
    pub(crate) fn is_full(&self) -> bool {
        self.next_free_index >= CAPACITY
    }

    fn entry(&self, index: usize) -> &Entry<T> {
        let entry_ptr = self.entry_ptr(index);

        // SAFETY: We ensured in the ctor that every entry is initialized and ensured above
        // that the pointer is valid, so we can safely dereference it.
        unsafe { entry_ptr.as_ref() }
    }

    #[expect(clippy::needless_pass_by_ref_mut, reason = "false positive")]
    fn entry_mut(&mut self, index: usize) -> &mut Entry<T> {
        let mut entry_ptr = self.entry_ptr(index);

        // SAFETY: We ensured in the ctor that every entry is initialized and ensured above
        // that the pointer is valid, so we can safely dereference it.
        unsafe { entry_ptr.as_mut() }
    }

    fn entry_ptr(&self, index: usize) -> NonNull<Entry<T>> {
        // This is only debug_assert because we do not rely on this for public API correctness.
        // The pool's ItemCoordinates logic guarantees that the per-slab indexing is in-bounds.
        debug_assert!(
            index < CAPACITY,
            "entry {index} index out of bounds in slab of {}",
            type_name::<T>()
        );

        // SAFETY: Guarded by bounds check above, so we are guaranteed that the pointer is valid.
        unsafe { self.first_entry_ptr.add(index) }
    }

    /// # Panics
    ///
    /// Panics if the index is out of bounds or is not associated with an item.
    #[must_use]
    pub(crate) fn get(&self, index: usize) -> Pin<&T> {
        match self.entry(index) {
            Entry::Occupied { value } => {
                // SAFETY: The value is guaranteed to be initialized because we only create
                // Occupied entries with properly initialized values.
                let init_ref = unsafe { value.assume_init_ref() };
                // SAFETY: This collection guarantees pinning. At no point do we
                // provide non-pinned references to the items.
                unsafe { Pin::new_unchecked(init_ref) }
            }
            Entry::Vacant { .. } => panic!(
                "get({index}) entry was vacant in slab of {}",
                type_name::<T>()
            ),
        }
    }

    /// # Panics
    ///
    /// Panics if the index is out of bounds or is not associated with an item.
    #[must_use]
    pub(crate) fn get_mut(&mut self, index: usize) -> Pin<&mut T> {
        match self.entry_mut(index) {
            Entry::Occupied { value } => {
                // SAFETY: The value is guaranteed to be initialized because we only create
                // Occupied entries with properly initialized values.
                let init_mut = unsafe { value.assume_init_mut() };
                // SAFETY: This collection guarantees pinning. At no point do we
                // provide non-pinned references to the items.
                unsafe { Pin::new_unchecked(init_mut) }
            }
            Entry::Vacant { .. } => panic!(
                "get_mut({index}) entry was vacant in slab of {}",
                type_name::<T>()
            ),
        }
    }

    /// # Panics
    ///
    /// Panics if the collection is full.
    #[must_use]
    pub(crate) fn begin_insert<'s, 'i>(&'s mut self) -> PinnedSlabInserter<'i, T, CAPACITY>
    where
        's: 'i,
    {
        #[cfg(debug_assertions)]
        self.integrity_check();

        assert!(
            !self.is_full(),
            "cannot insert into a full slab of {}",
            type_name::<T>()
        );

        // Pop the next free index from the stack of free entries.
        let next_free_index = self.next_free_index;

        PinnedSlabInserter {
            slab: self,
            index: next_free_index,
        }
    }

    /// # Panics
    ///
    /// Panics if the collection is full.
    #[must_use]
    #[allow(
        dead_code,
        reason = "not used for now but likely will be if we expose parent publicly"
    )]
    pub(crate) fn insert(&mut self, value: T) -> usize {
        let inserter = self.begin_insert();
        let index = inserter.index();
        inserter.insert(value);
        index
    }

    /// # Panics
    ///
    /// Panics if the index is out of bounds or is not associated with an item.
    pub(crate) fn remove(&mut self, index: usize) {
        let next_free_index = self.next_free_index;

        {
            let entry = self.entry_mut(index);

            match entry {
                Entry::Occupied { value } => {
                    // SAFETY: The value is guaranteed to be initialized because we only create
                    // Occupied entries with properly initialized values.
                    unsafe {
                        value.assume_init_drop();
                    }
                }
                Entry::Vacant { .. } => panic!(
                    "remove({index}) entry was vacant in slab of {}",
                    type_name::<T>()
                ),
            }

            *entry = Entry::Vacant { next_free_index };
        }

        // Push the removed item's entry onto the free stack.
        self.next_free_index = index;

        // We would have panicked above if the entry were not occupied,
        // so we really did remove an item and this cannot underflow.
        self.count = self.count.wrapping_sub(1);
    }

    /// Iterates through the items in the slab.
    pub(crate) fn iter(&self) -> PinnedSlabIterator<'_, T, CAPACITY> {
        PinnedSlabIterator::new(self)
    }

    #[cfg_attr(test, mutants::skip)] // This is essentially test logic, mutation is meaningless.
    #[cfg(debug_assertions)]
    pub(crate) fn integrity_check(&self) {
        let mut observed_is_vacant: [Option<bool>; CAPACITY] = [None; CAPACITY];
        let mut observed_next_free_index: [Option<usize>; CAPACITY] = [None; CAPACITY];
        let mut observed_occupied_count: usize = 0;

        for index in 0..CAPACITY {
            match self.entry(index) {
                Entry::Occupied { .. } => {
                    *observed_is_vacant
                        .get_mut(index)
                        .expect("guarded by loop range") = Some(false);
                    observed_occupied_count = observed_occupied_count
                        .checked_add(1)
                        .expect("guarded by capacity < usize::MAX in slab ctor");
                }
                Entry::Vacant { next_free_index } => {
                    *observed_is_vacant
                        .get_mut(index)
                        .expect("guarded by loop range") = Some(true);
                    *observed_next_free_index
                        .get_mut(index)
                        .expect("guarded by loop range") = Some(*next_free_index);
                }
            }
        }

        assert!(
            matches!(
                observed_is_vacant.get(self.next_free_index),
                None | Some(Some(true))
            ),
            "self.next_free_index points to an occupied slot {} in slab of {}",
            self.next_free_index,
            type_name::<T>()
        );

        assert!(
            self.count == observed_occupied_count,
            "self.count {} does not match the observed occupied count {} in slab of {}",
            self.count,
            observed_occupied_count,
            type_name::<T>()
        );

        // Verify that all vacant entries are valid.
        for index in 0..CAPACITY {
            if !observed_is_vacant
                .get(index)
                .expect("guarded by loop range")
                .unwrap()
            {
                continue;
            }

            let next_free_index = observed_next_free_index
                .get(index)
                .expect("guarded by loop range")
                .unwrap();

            if next_free_index == CAPACITY {
                // This is fine - it means the slab became full once we inserted this one.
                continue;
            }

            assert!(
                next_free_index <= CAPACITY,
                "entry {} is vacant but has an out-of-bounds next_free_index beyond COUNT {} in slab of {}",
                index,
                next_free_index,
                type_name::<T>()
            );

            assert!(
                observed_is_vacant
                    .get(next_free_index)
                    .expect("guarded by previous assertion")
                    .unwrap(),
                "entry {} is vacant but its next_free_index {} points to an occupied slot in slab of {}",
                index,
                next_free_index,
                type_name::<T>()
            );
        }
    }
}

impl<T, const CAPACITY: usize> Drop for PinnedSlab<T, CAPACITY> {
    fn drop(&mut self) {
        let was_empty = self.is_empty();

        // Set them all to `Vacant` to drop any occupied data.
        for index in 0..CAPACITY {
            let entry = self.entry_mut(index);

            // Drop the value if it's occupied before replacing with vacant.
            if let Entry::Occupied { value } = entry {
                // SAFETY: The value is guaranteed to be initialized because we only create
                // Occupied entries with properly initialized values.
                unsafe {
                    value.assume_init_drop();
                }
            }

            *entry = Entry::Vacant {
                // Intentionally anomalous - we are dropping so do not expect any more usage.
                next_free_index: usize::MAX,
            };
        }

        // SAFETY: The layout must match between alloc and dealloc. It does.
        unsafe {
            dealloc(self.first_entry_ptr.as_ptr().cast(), Self::layout());
        }

        // We do this check at the end so we clean up the memory first. Mostly to make Miri happy.
        // As we are going to panic anyway if something is wrong, there is little good to expect
        // for the app itself.
        //
        // If we are already panicking, we do not want to panic again because that will
        // simply obscure whatever the original panic was, leading to debug difficulties.
        if self.drop_policy == DropPolicy::MustNotDropItems && !thread::panicking() {
            assert!(
                was_empty,
                "dropped a non-empty slab of {} with a policy that says it must be empty when dropped",
                type_name::<T>()
            );
        }
    }
}

// SAFETY: Yes, there are raw pointers involved here but nothing inherently non-thread-mobile
// about it, so as long as T itself can move between threads, the slab can do so, too.
unsafe impl<T: Send, const CAPACITY: usize> Send for PinnedSlab<T, CAPACITY> {}

#[derive(Debug)]
pub(crate) struct PinnedSlabInserter<'s, T, const CAPACITY: usize> {
    slab: &'s mut PinnedSlab<T, CAPACITY>,

    /// Index at which the item will be inserted.
    index: usize,
}

impl<'s, T, const CAPACITY: usize> PinnedSlabInserter<'s, T, CAPACITY> {
    #[must_use]
    pub(crate) fn index(&self) -> usize {
        self.index
    }

    pub(crate) fn insert<'v>(self, value: T) -> Pin<&'v T>
    where
        's: 'v,
    {
        // Inserting an item always results in an exclusive reference, so this non-mut method
        // simply downgrades the exclusive reference to a shared one.
        self.insert_mut(value).into_ref()
    }

    pub(crate) fn insert_mut<'v>(self, value: T) -> Pin<&'v mut T>
    where
        's: 'v,
    {
        // SAFETY: We properly initialize the value by writing to it.
        unsafe {
            self.insert_with_mut(|uninit| {
                uninit.write(value);
            })
        }
    }

    /// # Safety
    ///
    /// The closure must properly initialize the `MaybeUninit<T>` before returning.
    /// Failure to do so will result in undefined behavior when the value is later accessed.
    #[allow(dead_code, reason = "not used yet but provides the non-mut variant")]
    pub(crate) unsafe fn insert_with<'v>(self, f: impl FnOnce(&mut MaybeUninit<T>)) -> Pin<&'v T>
    where
        's: 'v,
    {
        // SAFETY: Caller guarantees that the closure properly initializes the value.
        unsafe { self.insert_with_mut(f).into_ref() }
    }

    /// # Safety
    ///
    /// The closure must properly initialize the `MaybeUninit<T>` before returning.
    /// Failure to do so will result in undefined behavior when the value is later accessed.
    pub(crate) unsafe fn insert_with_mut<'v>(
        self,
        f: impl FnOnce(&mut MaybeUninit<T>),
    ) -> Pin<&'v mut T>
    where
        's: 'v,
    {
        let mut entry_ptr = self.slab.entry_ptr(self.index);

        // This detaches the lifetime of the slab from the lifetime of the entry for the purpose
        // of this method. We restore the relationship for the caller via function signature.
        //
        // This is because we have to return a reference to the filled entry, which borrows the slab
        // and thereby locks the slab. However, in this function that would prevent the slab field
        // updates we need to do.
        //
        // SAFETY: We are not allowed to perform operations on the slab that would create another
        // reference to the entry (because we hold an exclusive reference). We do not do that, and
        // the slab by design does not create/hold permanent references to its entries.
        let entry = unsafe { entry_ptr.as_mut() };

        let previous_entry = mem::replace(
            entry,
            Entry::Occupied {
                value: MaybeUninit::uninit(),
            },
        );

        self.slab.next_free_index = match previous_entry {
            Entry::Vacant { next_free_index } => next_free_index,
            Entry::Occupied { .. } => panic!(
                "entry {} was not vacant when we inserted into it in slab of {}",
                self.index,
                type_name::<T>()
            ),
        };

        // Initialize the value using the closure.
        let value = match entry {
            Entry::Occupied { value } => {
                f(value);
                value
            }
            Entry::Vacant { .. } => panic!(
                "entry {} was not occupied after we inserted into it in slab of {}",
                self.index,
                type_name::<T>()
            ),
        };

        // SAFETY: The value is guaranteed to be initialized because the caller's closure
        // was required to initialize it.
        let init_mut = unsafe { value.assume_init_mut() };
        // SAFETY: Items are always pinned - that is the point of this collection.
        let pinned_ref: Pin<&'v mut T> = unsafe { Pin::new_unchecked(init_mut) };

        // usize overflow implies we have filled all of virtual memory - never going to happen.
        self.slab.count = self.slab.count.wrapping_add(1);

        pinned_ref
    }
}

#[derive(Debug)]
#[must_use]
pub(crate) struct PinnedSlabIterator<'s, T, const CAPACITY: usize> {
    slab: &'s PinnedSlab<T, CAPACITY>,
    current_index: usize,
}

impl<'s, T, const CAPACITY: usize> PinnedSlabIterator<'s, T, CAPACITY> {
    fn new(slab: &'s PinnedSlab<T, CAPACITY>) -> Self {
        Self {
            slab,
            current_index: 0,
        }
    }
}

impl<'s, T, const CAPACITY: usize> Iterator for PinnedSlabIterator<'s, T, CAPACITY> {
    type Item = Pin<&'s T>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.current_index < CAPACITY {
            let entry_index = self.current_index;
            self.current_index = self
                .current_index
                .checked_add(1)
                .expect("guarded by capacity < usize::MAX in slab ctor");

            let entry = self.slab.entry(entry_index);

            if let Entry::Occupied { .. } = entry {
                return Some(self.slab.get(entry_index));
            }
        }

        None
    }
}

/// Ensures that the virtual memory pages that are behind a pointer are mapped to physical pages.
///
/// The operating system normally maps virtual pages to physical pages on-demand. However, this
/// has some caveats: it may happen on hot paths and inside critical sections, which is not
/// desirable; furthermore, some operating system APIs (e.g. reading from a Windows socket)
/// will switch to a slower logic path if provided virtual pages that are not yet mapped to
/// physical pages. Therefore, it is beneficial to ensure memory is mapped to physical pages
/// ahead of time if it is known to eventually be used.
///
/// We are operating in context of an object pool, so we can assume all its memory will be used,
/// so we do this eagerly when extending pool capacity.
///
/// We implement this by writing to each page of memory in the pointer's range, so the memory
/// is assumed to be uninitialized. Calling this on initialized memory would conceptually be
/// pointless, anyway.
#[cfg_attr(test, mutants::skip)] // Impractical to test, as it is merely an optimization that is rarely visible.
fn ensure_virtual_pages_mapped_to_physical_pages<T, const COUNT: usize>(ptr: NonNull<T>) {
    if size_of::<T>() < 4096 {
        // If this is smaller than a typical page, then merely initializing the `Entry::Vacant`
        // will be enough to touch every page and we can skip this optimization.
        return;
    }

    // SAFETY: This is the slab pointer as given by the caller, it must be valid.
    // Note that the count is in units of T, not in bytes.
    unsafe {
        ptr.write_bytes(0x3F, COUNT);
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        reason = "we do not need to worry about these things when writing test code"
    )]

    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};
    use std::thread;

    use super::*;

    #[test]
    fn smoke_test() {
        let mut slab = PinnedSlab::<u32, 3>::new(DropPolicy::MayDropItems);

        let index_a = slab.insert(42);
        let index_b = slab.insert(43);
        let index_c = slab.insert(44);

        assert_eq!(*slab.get(index_a), 42);
        assert_eq!(*slab.get(index_b), 43);
        assert_eq!(*slab.get(index_c), 44);

        assert_eq!(slab.len(), 3);

        slab.remove(index_b);

        assert_eq!(slab.len(), 2);

        let index_d = slab.insert(45);

        assert_eq!(*slab.get(index_a), 42);
        assert_eq!(*slab.get(index_c), 44);
        assert_eq!(*slab.get(index_d), 45);

        assert!(slab.is_full());
    }

    #[test]
    #[should_panic]
    fn panic_when_full() {
        let mut slab = PinnedSlab::<u32, 3>::new(DropPolicy::MayDropItems);

        _ = slab.insert(42);
        _ = slab.insert(43);
        _ = slab.insert(44);

        _ = slab.insert(45);
    }

    #[test]
    #[should_panic]
    fn panic_when_oob_get() {
        let mut slab = PinnedSlab::<u32, 3>::new(DropPolicy::MayDropItems);

        _ = slab.insert(42);
        _ = slab.get(1234);
    }

    #[test]
    fn begin_insert_returns_correct_index() {
        let mut slab = PinnedSlab::<u32, 3>::new(DropPolicy::MayDropItems);

        // We expect that we insert items in order, from the start (0, 1, 2, ...).

        let inserter = slab.begin_insert();
        assert_eq!(inserter.index(), 0);
        inserter.insert(10);
        assert_eq!(*slab.get(0), 10);

        let inserter = slab.begin_insert();
        assert_eq!(inserter.index(), 1);
        inserter.insert(11);
        assert_eq!(*slab.get(1), 11);

        let inserter = slab.begin_insert();
        assert_eq!(inserter.index(), 2);
        inserter.insert(12);
        assert_eq!(*slab.get(2), 12);
    }

    #[test]
    fn abandoned_inserter_is_noop() {
        let mut slab = PinnedSlab::<u32, 3>::new(DropPolicy::MayDropItems);

        // If you abandon an inserter, nothing happens.
        let inserter = slab.begin_insert();
        assert_eq!(inserter.index(), 0);

        let inserter = slab.begin_insert();
        assert_eq!(inserter.index(), 0);
        inserter.insert(20);

        assert_eq!(*slab.get(0), 20);

        // There must still be room for 2 more.
        _ = slab.insert(123);
        _ = slab.insert(456);
    }

    #[test]
    fn remove_makes_room() {
        let mut slab = PinnedSlab::<u32, 3>::new(DropPolicy::MayDropItems);

        let a = slab.insert(42);
        let b = slab.insert(43);
        let c = slab.insert(44);

        slab.remove(b);

        let d = slab.insert(45);

        assert_eq!(*slab.get(a), 42);
        assert_eq!(*slab.get(c), 44);
        assert_eq!(*slab.get(d), 45);
    }

    #[test]
    #[should_panic]
    fn remove_vacant_panics() {
        let mut slab = PinnedSlab::<u32, 3>::new(DropPolicy::MayDropItems);

        slab.remove(1);
    }

    #[test]
    #[should_panic]
    fn get_vacant_panics() {
        let slab = PinnedSlab::<u32, 3>::new(DropPolicy::MayDropItems);

        _ = slab.get(1);
    }

    #[test]
    #[should_panic]
    fn get_mut_vacant_panics() {
        let mut slab = PinnedSlab::<u32, 3>::new(DropPolicy::MayDropItems);

        _ = slab.get_mut(1);
    }

    #[test]
    fn in_refcell_works_fine() {
        let slab = RefCell::new(PinnedSlab::<u32, 3>::new(DropPolicy::MayDropItems));

        {
            let mut slab = slab.borrow_mut();
            let a = slab.insert(42);
            let b = slab.insert(43);
            let c = slab.insert(44);

            assert_eq!(*slab.get(a), 42);
            assert_eq!(*slab.get(b), 43);
            assert_eq!(*slab.get(c), 44);

            slab.remove(b);

            let d = slab.insert(45);

            assert_eq!(*slab.get(a), 42);
            assert_eq!(*slab.get(c), 44);
            assert_eq!(*slab.get(d), 45);
        }

        {
            let slab = slab.borrow();
            assert_eq!(*slab.get(0), 42);
            assert!(slab.is_full());
        }
    }

    #[test]
    fn calls_drop_on_remove() {
        struct Droppable {
            dropped: Rc<Cell<bool>>,
        }

        impl Drop for Droppable {
            fn drop(&mut self) {
                self.dropped.set(true);
            }
        }

        let dropped = Rc::new(Cell::new(false));
        let mut slab = PinnedSlab::<Droppable, 3>::new(DropPolicy::MayDropItems);

        let a = slab.insert(Droppable {
            dropped: Rc::clone(&dropped),
        });
        slab.remove(a);

        assert!(dropped.get());
    }

    #[test]
    fn multithreaded_via_mutex() {
        let slab = Arc::new(Mutex::new(PinnedSlab::<u32, 3>::new(
            DropPolicy::MayDropItems,
        )));

        let a;
        let b;
        let c;

        {
            let mut slab = slab.lock().unwrap();
            a = slab.insert(42);
            b = slab.insert(43);
            c = slab.insert(44);

            assert_eq!(*slab.get(a), 42);
            assert_eq!(*slab.get(b), 43);
            assert_eq!(*slab.get(c), 44);
        }

        let slab_clone = Arc::clone(&slab);
        thread::spawn(move || {
            let mut slab = slab_clone.lock().unwrap();

            slab.remove(b);

            let d = slab.insert(45);

            assert_eq!(*slab.get(a), 42);
            assert_eq!(*slab.get(c), 44);
            assert_eq!(*slab.get(d), 45);
        });

        let slab = slab.lock().unwrap();
        assert!(slab.is_full());
    }

    #[test]
    #[should_panic]
    fn drop_item_with_forbidden_to_drop_policy_panics() {
        let mut slab = PinnedSlab::<u32, 3>::new(DropPolicy::MustNotDropItems);
        _ = slab.insert(123);
    }

    #[test]
    fn drop_itemless_with_forbidden_to_drop_policy_ok() {
        drop(PinnedSlab::<u32, 3>::new(DropPolicy::MustNotDropItems));
    }

    #[test]
    #[should_panic]
    fn zst_is_panic() {
        drop(PinnedSlab::<(), 3>::new(DropPolicy::MayDropItems));
    }

    #[test]
    #[should_panic]
    fn zero_capacity_is_panic() {
        drop(PinnedSlab::<usize, 0>::new(DropPolicy::MayDropItems));
    }

    #[test]
    fn insert_with_mut_works() {
        let mut slab = PinnedSlab::<u32, 3>::new(DropPolicy::MayDropItems);

        let inserter = slab.begin_insert();
        let index = inserter.index();

        // SAFETY: We properly initialize the value in the closure.
        let value_ref = unsafe {
            inserter.insert_with_mut(|uninit| {
                uninit.write(42);
            })
        };

        assert_eq!(*value_ref, 42);
        assert_eq!(*slab.get(index), 42);
        assert_eq!(slab.len(), 1);
    }

    #[test]
    fn insert_with_allows_complex_initialization() {
        struct ComplexType {
            field1: u32,
            field2: String,
        }

        let mut slab = PinnedSlab::<ComplexType, 3>::new(DropPolicy::MayDropItems);

        let inserter = slab.begin_insert();
        let index = inserter.index();

        // SAFETY: We properly initialize the value in the closure.
        let value_ref = unsafe {
            inserter.insert_with_mut(|uninit| {
                uninit.write(ComplexType {
                    field1: 123,
                    field2: String::from("hello"),
                });
            })
        };

        assert_eq!(value_ref.field1, 123);
        assert_eq!(value_ref.field2, "hello");

        let retrieved = slab.get(index);
        assert_eq!(retrieved.field1, 123);
        assert_eq!(retrieved.field2, "hello");
    }

    #[test]
    fn insert_with_returns_shared_ref() {
        let mut slab = PinnedSlab::<u32, 3>::new(DropPolicy::MayDropItems);

        let inserter = slab.begin_insert();
        let index = inserter.index();

        // SAFETY: We properly initialize the value in the closure.
        let value_ref = unsafe {
            inserter.insert_with(|uninit| {
                uninit.write(789);
            })
        };

        assert_eq!(*value_ref, 789);
        assert_eq!(*slab.get(index), 789);
        assert_eq!(slab.len(), 1);
    }

    #[test]
    fn insert_with_partial_initialization() {
        use std::mem::MaybeUninit;

        #[allow(
            dead_code,
            reason = "memory field is used for demonstration but not accessed in test"
        )]
        struct HalfFull {
            value: usize,
            memory: [MaybeUninit<u8>; 16],
        }

        /// Helper function to initialize only the value field of `HalfFull`.
        /// This separates unsafe operations into individual blocks for Clippy compliance.
        fn initialize_half_full(uninit: &mut MaybeUninit<HalfFull>) {
            let ptr = uninit.as_mut_ptr();

            // SAFETY: ptr points to valid memory and we're dereferencing to take address of a field.
            let value_ptr = unsafe { &raw mut (*ptr).value };

            // SAFETY: value_ptr points to valid memory for a usize.
            unsafe {
                value_ptr.write(42);
            }

            // Note: We deliberately do NOT write to the memory field at all.
            // It remains truly uninitialized, which is safe because MaybeUninit<T>
            // does not require its contents to be initialized.
        }

        let mut slab = PinnedSlab::<HalfFull, 3>::new(DropPolicy::MayDropItems);

        let inserter = slab.begin_insert();
        let index = inserter.index();

        // SAFETY: We properly initialize only the required fields via the closure.
        let value_ref = unsafe {
            inserter.insert_with(|uninit| {
                initialize_half_full(uninit);
            })
        };

        assert_eq!(value_ref.value, 42);

        let retrieved = slab.get(index);
        assert_eq!(retrieved.value, 42);
        assert_eq!(slab.len(), 1);
    }

    #[test]
    fn iter_empty_slab() {
        let slab = PinnedSlab::<u32, 3>::new(DropPolicy::MayDropItems);
        let mut iter = slab.iter();

        assert!(iter.next().is_none());
    }

    #[test]
    fn iter_single_item() {
        let mut slab = PinnedSlab::<String, 3>::new(DropPolicy::MayDropItems);
        let _index = slab.insert("hello".to_string());

        let items: Vec<_> = slab.iter().collect();
        assert_eq!(items.len(), 1);
        assert_eq!(&*items[0], "hello");
    }

    #[test]
    fn iter_multiple_items() {
        let mut slab = PinnedSlab::<i32, 5>::new(DropPolicy::MayDropItems);
        let _idx1 = slab.insert(10);
        let _idx2 = slab.insert(20);
        let _idx3 = slab.insert(30);

        let items: Vec<_> = slab.iter().map(|item| *item).collect();
        assert_eq!(items.len(), 3);

        // Items should be iterated in insertion order
        assert_eq!(items[0], 10);
        assert_eq!(items[1], 20);
        assert_eq!(items[2], 30);
    }

    #[test]
    fn iter_with_gaps() {
        let mut slab = PinnedSlab::<u64, 5>::new(DropPolicy::MayDropItems);
        let _idx1 = slab.insert(100);
        let idx2 = slab.insert(200);
        let idx3 = slab.insert(300);
        let _idx4 = slab.insert(400);

        // Remove middle items to create gaps
        slab.remove(idx2);
        slab.remove(idx3);

        let items: Vec<_> = slab.iter().map(|item| *item).collect();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0], 100);
        assert_eq!(items[1], 400);
    }

    #[test]
    fn iter_full_slab() {
        let mut slab = PinnedSlab::<usize, 3>::new(DropPolicy::MayDropItems);
        let _idx1 = slab.insert(1);
        let _idx2 = slab.insert(2);
        let _idx3 = slab.insert(3);

        assert!(slab.is_full());

        let items: Vec<_> = slab.iter().map(|item| *item).collect();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], 1);
        assert_eq!(items[1], 2);
        assert_eq!(items[2], 3);
    }

    #[test]
    fn iter_multiple_iterators() {
        let mut slab = PinnedSlab::<u8, 3>::new(DropPolicy::MayDropItems);
        let _idx1 = slab.insert(1);
        let _idx2 = slab.insert(2);

        // Should be able to create multiple iterators
        let iter1 = slab.iter();
        let iter2 = slab.iter();

        let items1: Vec<_> = iter1.map(|item| *item).collect();
        let items2: Vec<_> = iter2.map(|item| *item).collect();

        assert_eq!(items1, items2);
        assert_eq!(items1, vec![1, 2]);
    }

    #[test]
    fn large_item() {
        let mut slab = PinnedSlab::<[u8; 10240], 123>::new(DropPolicy::MayDropItems);

        let index = slab.insert([88u8; 10240]);
        slab.remove(index);
    }
}
