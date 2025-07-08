use core::panic;
use std::alloc::{Layout, alloc, dealloc};
use std::any::type_name;
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
    Occupied { value: T },

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
        assert!(
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
            // SAFETY: This collection guarantees pinning. At no point do we
            // provide non-pinned references to the items.
            Entry::Occupied { value } => unsafe { Pin::new_unchecked(value) },
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
            // SAFETY: This collection guarantees pinning. At no point do we
            // provide non-pinned references to the items.
            Entry::Occupied { value } => unsafe { Pin::new_unchecked(value) },
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

            if matches!(entry, Entry::Vacant { .. }) {
                panic!(
                    "remove({index}) entry was vacant in slab of {}",
                    type_name::<T>()
                );
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

        let previous_entry = mem::replace(entry, Entry::Occupied { value });

        self.slab.next_free_index = match previous_entry {
            Entry::Vacant { next_free_index } => next_free_index,
            Entry::Occupied { .. } => panic!(
                "entry {} was not vacant when we inserted into it in slab of {}",
                self.index,
                type_name::<T>()
            ),
        };

        let pinned_ref: Pin<&'v mut T> = match entry {
            // SAFETY: Items are always pinned - that is the point of this collection.
            Entry::Occupied { value } => unsafe { Pin::new_unchecked(value) },
            Entry::Vacant { .. } => panic!(
                "entry {} was not occupied after we inserted into it in slab of {}",
                self.index,
                type_name::<T>()
            ),
        };

        self.slab.count = self
            .slab
            .count
            .checked_add(1)
            .expect("guarded by capacity < usize::MAX in slab ctor");

        pinned_ref
    }
}

#[cfg(test)]
mod tests {
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
}
