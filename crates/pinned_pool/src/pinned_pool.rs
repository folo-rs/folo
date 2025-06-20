use num::Integer;

use crate::{DropPolicy, PinnedPoolBuilder, PinnedSlab, PinnedSlabInserter};
use std::pin::Pin;

/// An object pool of unbounded size that guarantees pinning of its items.
///
/// There are multiple ways to insert items into the collection:
///
/// * [`insert()`][3] - inserts a value and returns the key. This is the simplest way to add an
///   item but requires you to later look it up by the key. That lookup is fast but not free.
/// * [`begin_insert().insert()`][4] - returns a shared reference to the inserted item; you may
///   also obtain the key in advance from the inserter through [`key()`][7] which may be
///   useful if the item needs to know its own key in the collection.
/// * [`begin_insert().insert_mut()`][5] - returns an exclusive reference to the inserted item; you
///   may also obtain the key in advance from the inserter through [`key()`][7] which may be
///   useful if the item needs to know its own key in the collection.
///
/// The pool returns a key for each inserted item, with items on an operating being keyed by this.
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
/// # Multithreaded usage
///  
/// To share the collection between threads, wrapping in `Mutex` is the recommended approach.
///
/// # Pinning
///
/// The collection itself does not need to be pinned - only the contents are pinned.
///
/// # Resource usage
///
/// As of today, the collection never shrinks, though future versions may offer facilities to do so.
///
/// [1]: Self::get
/// [2]: Self::get_mut
/// [3]: Self::insert
/// [4]: PinnedPoolInserter::insert
/// [5]: PinnedPoolInserter::insert_mut
/// [7]: PinnedPoolInserter::key
#[derive(Debug)]
pub struct PinnedPool<T> {
    /// The slabs that provide the storage of the pool.
    /// We use a Vec here to allow for dynamic capacity growth.
    ///
    /// For now, we only grow this Vec but in theory, one could implement shrinking as well
    /// by removing empty slabs (we cannot touch non-empty slabs because we made a promise to pin).
    slabs: Vec<PinnedSlab<T, SLAB_CAPACITY>>,

    /// Lowest index of any slab that has a vacant slot, if known. We use this to avoid scanning
    /// the entire collection for vacant slots when inserting an item. This being `None` does not
    /// imply that there are no vacant slots, it just means we do not know what slab they are in.
    /// In other words, this is a cache, not the ground truth - we set it to `None` when we lose
    /// confidence that the data is still valid but when we have no need to look up the new value.
    slab_with_vacant_slot_index: Option<usize>,

    drop_policy: DropPolicy,
}

/// A key that can be used to reference up an item in a [`PinnedPool`].
///
/// Keys may be reused by the pool after an item is removed.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Key {
    index_in_pool: usize,
}

/// Today, we assemble the pool from pinned slabs, each containing a fixed number of items.
///
/// In the future, we may choose to be smarter about this, e.g. choosing the slab size dynamically
/// based on the size of T in order to match a memory page size, or another similar criterion.
/// This is why the parameter is also not exposed in the public API - we may want to change how we
/// perform the memory layout in a future version.
const SLAB_CAPACITY: usize = 128;

impl<T> PinnedPool<T> {
    /// # Panics
    ///
    /// Panics if `T` is zero-sized.
    #[must_use]
    pub(crate) fn new_inner(drop_policy: DropPolicy) -> Self {
        assert!(
            size_of::<T>() > 0,
            "PinnedPool must have non-zero item size"
        );

        Self {
            slabs: Vec::new(),
            drop_policy,
            slab_with_vacant_slot_index: None,
        }
    }

    /// Creates a new [`PinnedPool`] with the default configuration.
    ///
    /// # Panics
    ///
    /// Panics if `T` is zero-sized.
    #[must_use]
    pub fn new() -> Self {
        Self::builder().build()
    }

    /// Starts building a new [`PinnedPool`].
    pub fn builder() -> PinnedPoolBuilder<T> {
        PinnedPoolBuilder::new()
    }

    /// The number of items in the pool.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slabs.iter().map(PinnedSlab::len).sum()
    }

    /// The number of items the pool can accommodate without additional resource allocation.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.slabs.len()
            .checked_mul(SLAB_CAPACITY)
            .expect("overflow here would mean the pool can hold more items than virtual memory can fit, which makes no sense - it would never grow that big")
    }

    /// Whether the pool is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slabs.iter().all(PinnedSlab::is_empty)
    }

    /// Gets a pinned reference to an item in the pool by its key.
    ///
    /// # Panics
    ///
    /// Panics if the key is not associated with an item.
    #[must_use]
    pub fn get(&self, key: Key) -> Pin<&T> {
        let coordinates = ItemCoordinates::<SLAB_CAPACITY>::from_key(key);

        self.slabs
            .get(coordinates.slab_index)
            .map(|s| s.get(coordinates.index_in_slab))
            .expect("key was not associated with an item in the pool")
    }

    /// Gets an exclusive pinned reference to an item in the pool by its key.
    ///
    /// # Panics
    ///
    /// Panics if the key is not associated with an item.
    #[must_use]
    pub fn get_mut(&mut self, key: Key) -> Pin<&mut T> {
        let index = ItemCoordinates::<SLAB_CAPACITY>::from_key(key);

        self.slabs
            .get_mut(index.slab_index)
            .map(|s| s.get_mut(index.index_in_slab))
            .expect("key was not associated with an item in the pool")
    }

    /// Creates an inserter that enables advanced techniques for inserting an item into the pool.
    ///
    /// For example, using an inserter allows you to obtain the key before the item is inserted
    /// and allows you to immediately obtain a pinned reference to the item.
    #[must_use]
    pub fn begin_insert<'a, 'b>(&'a mut self) -> PinnedPoolInserter<'b, T>
    where
        'a: 'b,
    {
        let slab_index = self.index_of_slab_with_vacant_slot();
        let slab = self
            .slabs
            .get_mut(slab_index)
            .expect("we just verified that there is a slab with a vacant slot at this index");

        // We invalidate the "slab with vacant slot" cache here if this is the last vacant slot.
        // It is true that just creating an inserter does not mean we will insert an item. After
        // all, the inserter may be abandoned. However, we do this invalidation preemptively
        // because Rust lifetimes make it hard to modify the pool from the inserter (as we are
        // already borrowing the slab exclusively). Since it is just a cache, this is no big deal.
        let predicted_slab_filled_slots = slab.len()
            .checked_add(1)
            .expect("we cannot overflow because there is at least one free slot, so it means there must be room to increment");

        if predicted_slab_filled_slots == SLAB_CAPACITY {
            self.slab_with_vacant_slot_index = None;
        }

        let slab_inserter = slab.begin_insert();

        PinnedPoolInserter {
            slab_inserter,
            slab_index,
        }
    }

    /// Inserts an item into the pool and returns its key.
    #[must_use]
    pub fn insert(&mut self, value: T) -> Key {
        let inserter = self.begin_insert();
        let key = inserter.key();
        inserter.insert(value);
        key
    }

    /// # Panics
    ///
    /// Panics if the key is not associated with an item.
    pub fn remove(&mut self, key: Key) {
        let index = ItemCoordinates::<SLAB_CAPACITY>::from_key(key);

        let Some(slab) = self.slabs.get_mut(index.slab_index) else {
            panic!("key was not associated with an item in the pool")
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
            self.slabs.push(PinnedSlab::new(self.drop_policy));

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

impl<T> Default for PinnedPool<T> {
    /// Creates a new [`PinnedPool`] with the default configuration.
    ///
    /// # Panics
    ///
    /// Panics if `T` is zero-sized.
    fn default() -> Self {
        Self::new()
    }
}

/// An inserter for a [`PinnedPool`], enabling more item insertion scenarios than afforded by
/// [`PinnedPool::insert()`][1].
///
/// [1]: PinnedPool::insert
#[derive(Debug)]
pub struct PinnedPoolInserter<'s, T> {
    slab_inserter: PinnedSlabInserter<'s, T, SLAB_CAPACITY>,
    slab_index: usize,
}

impl<'s, T> PinnedPoolInserter<'s, T> {
    /// Inserts an item and returns a pinned reference to it.
    pub fn insert<'v>(self, value: T) -> Pin<&'v T>
    where
        's: 'v,
    {
        self.slab_inserter.insert(value)
    }

    /// Inserts an item and returns a pinned exclusive reference to it.
    pub fn insert_mut<'v>(self, value: T) -> Pin<&'v mut T>
    where
        's: 'v,
    {
        self.slab_inserter.insert_mut(value)
    }

    /// The key of the item that will be inserted by this inserter.
    ///
    /// If the inserted is abandoned, the key may be used by a different item inserted later.
    #[must_use]
    pub fn key(&self) -> Key {
        ItemCoordinates::<SLAB_CAPACITY>::from_parts(self.slab_index, self.slab_inserter.index())
            .to_key()
    }
}

#[derive(Debug)]
struct ItemCoordinates<const SLAB_CAPACITY: usize> {
    slab_index: usize,
    index_in_slab: usize,
}

impl<const SLAB_CAPACITY: usize> ItemCoordinates<SLAB_CAPACITY> {
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
                .expect("key indicates an item beyond the range of virtual memory - impossible to reach this point from a valid history")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        cell::RefCell,
        ptr,
        sync::{Arc, Mutex},
        thread,
    };

    #[test]
    fn smoke_test() {
        let mut pool = PinnedPool::<u32>::new();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());

        let key_a = pool.insert(42);
        let key_b = pool.insert(43);
        let key_c = pool.insert(44);

        assert_eq!(pool.len(), 3);
        assert!(!pool.is_empty());
        assert!(pool.capacity() >= 3);

        assert_eq!(*pool.get(key_a), 42);
        assert_eq!(*pool.get(key_b), 43);
        assert_eq!(*pool.get(key_c), 44);

        pool.remove(key_b);

        let key_d = pool.insert(45);

        assert_eq!(*pool.get(key_a), 42);
        assert_eq!(*pool.get(key_c), 44);
        assert_eq!(*pool.get(key_d), 45);
    }

    #[test]
    #[should_panic]
    fn panic_when_empty_oob_get() {
        let pool = PinnedPool::<u32>::new();

        _ = pool.get(Key { index_in_pool: 0 });
    }

    #[test]
    #[should_panic]
    fn panic_when_oob_get() {
        let mut pool = PinnedPool::<u32>::new();

        _ = pool.insert(42);
        _ = pool.get(Key {
            index_in_pool: 1234,
        });
    }

    #[test]
    fn begin_insert_returns_correct_key() {
        let mut pool = PinnedPool::<u32>::new();

        // We expect that we insert items in order, from the start (0, 1, 2, ...).

        let inserter = pool.begin_insert();
        let key = inserter.key();
        assert_eq!(key.index_in_pool, 0);
        inserter.insert(10);
        assert_eq!(*pool.get(key), 10);

        let inserter = pool.begin_insert();
        let key = inserter.key();
        assert_eq!(key.index_in_pool, 1);
        inserter.insert(11);
        assert_eq!(*pool.get(key), 11);

        let inserter = pool.begin_insert();
        let key = inserter.key();
        assert_eq!(key.index_in_pool, 2);
        inserter.insert(12);
        assert_eq!(*pool.get(key), 12);
    }

    #[test]
    fn abandoned_inserter_is_noop() {
        let mut pool = PinnedPool::<u32>::new();

        // If you abandon an inserter, nothing happens.
        _ = pool.begin_insert();

        let inserter = pool.begin_insert();
        let key = inserter.key();
        inserter.insert(20);

        assert_eq!(*pool.get(key), 20);

        _ = pool.insert(123);
        _ = pool.insert(456);
    }

    #[test]
    #[should_panic]
    fn remove_empty_panics() {
        let mut pool = PinnedPool::<u32>::new();

        pool.remove(Key { index_in_pool: 0 });
    }

    #[test]
    #[should_panic]
    fn remove_vacant_panics() {
        let mut pool = PinnedPool::<u32>::new();

        // Ensure the first slab is created, so collection is not empty.
        _ = pool.insert(1234);

        // There is nothing at this index, though.
        pool.remove(Key { index_in_pool: 1 });
    }

    #[test]
    #[should_panic]
    fn remove_oob_panics() {
        let mut pool = PinnedPool::<u32>::new();

        // Ensure the first slab is created, so collection is not empty.
        _ = pool.insert(1234);

        // This index is not in a valid slab.
        pool.remove(Key {
            index_in_pool: 9999999,
        });
    }

    #[test]
    #[should_panic]
    fn get_vacant_panics() {
        let mut pool = PinnedPool::<u32>::new();

        // Ensure the first slab is created, so collection is not empty.
        _ = pool.insert(1234);

        // There is nothing at this index, though.
        _ = pool.get(Key { index_in_pool: 1 });
    }

    #[test]
    #[should_panic]
    fn get_mut_vacant_panics() {
        let mut pool = PinnedPool::<u32>::new();

        // Ensure the first slab is created, so collection is not empty.
        _ = pool.insert(1234);

        // There is nothing at this index, though.
        _ = pool.get_mut(Key { index_in_pool: 1 });
    }

    #[test]
    fn in_refcell_works_fine() {
        let pool = RefCell::new(PinnedPool::<u32>::new());

        let key_a = {
            let mut pool = pool.borrow_mut();
            let key_a = pool.insert(42);
            let key_b = pool.insert(43);
            let key_c = pool.insert(44);

            assert_eq!(*pool.get(key_a), 42);
            assert_eq!(*pool.get(key_b), 43);
            assert_eq!(*pool.get(key_c), 44);

            pool.remove(key_b);

            let key_d = pool.insert(45);

            assert_eq!(*pool.get(key_a), 42);
            assert_eq!(*pool.get(key_c), 44);
            assert_eq!(*pool.get(key_d), 45);

            key_a
        };

        {
            let pool = pool.borrow();
            assert_eq!(*pool.get(key_a), 42);
        }
    }

    #[test]
    fn multithreaded_via_mutex() {
        let shared_pool = Arc::new(Mutex::new(PinnedPool::<u32>::new()));

        let key_a;
        let key_b;
        let key_c;

        {
            let mut pool = shared_pool.lock().unwrap();
            key_a = pool.insert(42);
            key_b = pool.insert(43);
            key_c = pool.insert(44);

            assert_eq!(*pool.get(key_a), 42);
            assert_eq!(*pool.get(key_b), 43);
            assert_eq!(*pool.get(key_c), 44);
        }

        thread::spawn({
            let shared_pool = Arc::clone(&shared_pool);
            move || {
                let mut pool = shared_pool.lock().unwrap();

                pool.remove(key_b);

                let d = pool.insert(45);

                assert_eq!(*pool.get(key_a), 42);
                assert_eq!(*pool.get(key_c), 44);
                assert_eq!(*pool.get(d), 45);
            }
        });

        let chain = shared_pool.lock().unwrap();
        assert!(!chain.is_empty());
    }

    #[test]
    #[should_panic]
    fn drop_item_with_forbidden_to_drop_policy_panics() {
        let mut pool = PinnedPool::<u32>::builder()
            .drop_policy(DropPolicy::MustNotDropItems)
            .build();
        _ = pool.insert(123);
    }

    #[test]
    fn drop_itemless_with_forbidden_to_drop_policy_ok() {
        drop(
            PinnedPool::<u32>::builder()
                .drop_policy(DropPolicy::MustNotDropItems)
                .build(),
        );
    }

    #[test]
    fn out_of_band_access() {
        // We grab pointers to items and access them without having borrowed the pool itself.
        // This is valid because the pool does not keep references to the items. The test will
        // pass even if we do something invalid but Miri will catch it - this test exists for Miri.
        let mut pool = PinnedPool::<u32>::new();

        let key_a = pool.insert(42);

        // It is valid to access pool items directly via pointers, as long as you do
        // not attempt to concurrently access them via pool methods.
        let a_ptr = ptr::from_mut(pool.get_mut(key_a).get_mut());

        // Modify item directly - pool is not borrowed here.
        // SAFETY: The pool allows us to touch items out of band.
        unsafe {
            *a_ptr += 1;
        }

        // We can even have a pending insert while we touch the item out of band.
        let inserter = pool.begin_insert();

        // SAFETY: The pool allows us to touch items out of band.
        unsafe {
            *a_ptr += 1;
        }

        _ = inserter.insert(123);

        // After this, we are not allowed to touch this item, because we have removed it.
        // That is, a_ptr now points to invalid memory. The pool does not know anything about
        // it, just our pointer is no longer valid for reads or writes - everything is out of band.
        pool.remove(key_a);
    }
}
