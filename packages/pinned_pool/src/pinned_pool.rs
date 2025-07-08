use std::pin::Pin;

use num::Integer;

use crate::{DropPolicy, PinnedPoolBuilder, PinnedSlab, PinnedSlabInserter};

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
/// # Resource usage
///
/// The collection automatically grows as items are added. To reduce memory usage after items have
/// been removed, use the [`shrink_to_fit()`][6] method to release unused capacity.
///
/// [1]: Self::get
/// [2]: Self::get_mut
/// [3]: Self::insert
/// [4]: PinnedPoolInserter::insert
/// [5]: PinnedPoolInserter::insert_mut
/// [6]: Self::shrink_to_fit
/// [7]: PinnedPoolInserter::key
#[derive(Debug)]
pub struct PinnedPool<T> {
    /// The slabs that provide the storage of the pool.
    /// We use a Vec here to allow for dynamic capacity growth.
    ///
    /// The Vec can grow as items are added and can shrink when empty slabs are removed via
    /// `shrink_to_fit()`. We cannot remove non-empty slabs because we made a promise to pin.
    slabs: Vec<PinnedSlab<T, SLAB_CAPACITY>>,

    /// Lowest index of any slab that has a vacant slot, if known. We use this to avoid scanning
    /// the entire collection for vacant slots when inserting an item. This being `None` does not
    /// imply that there are no vacant slots, it just means we do not know what slab they are in.
    /// In other words, this is a cache, not the ground truth - we set it to `None` when we lose
    /// confidence that the data is still valid but when we have no need to look up the new value.
    slab_with_vacant_slot_index: Option<usize>,

    drop_policy: DropPolicy,
}

/// A key that can be used to reference an item in a [`PinnedPool`].
///
/// Keys are opaque handles returned by [`PinnedPool::insert()`] and related methods.
/// They provide efficient access to items in the pool via [`PinnedPool::get()`] and
/// [`PinnedPool::get_mut()`].
///
/// # Key Reuse
///
/// Keys may be reused by the pool after an item is removed. This means that using a key
/// after its associated item has been removed may access a different item or panic.
///
/// # Example
///
/// ```rust
/// use pinned_pool::{Key, PinnedPool};
///
/// let mut pool = PinnedPool::<i32>::new();
///
/// // Insert items and store their keys
/// let key1 = pool.insert(42);
/// let key2 = pool.insert(24);
///
/// // Keys can be copied and stored
/// let stored_keys = vec![key1, key2];
///
/// // Use keys to access items
/// for &key in &stored_keys {
///     let item = pool.get(key);
///     println!("Item: {}", *item);
/// }
/// # pool.remove(key1);
/// # pool.remove(key2);
/// ```
///
/// Keys may be reused by the pool after an item is removed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Key {
    index_in_pool: usize,
}

/// Today, we assemble the pool from pinned slabs, each containing a fixed number of items.
///
/// In the future, we may choose to be smarter about this, e.g. choosing the slab size dynamically
/// based on the size of T in order to match a memory page size, or another similar criterion.
/// This is why the parameter is also not exposed in the public API - we may want to change how we
/// perform the memory layout in a future version.
#[cfg(not(miri))]
const SLAB_CAPACITY: usize = 128;

// Under Miri, we use a smaller slab capacity because Miri test runtime scales by memory usage.
#[cfg(miri)]
const SLAB_CAPACITY: usize = 4;

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
    /// The pool starts empty and will automatically grow as needed when items are inserted.
    ///
    /// # Example
    ///
    /// ```rust
    /// use pinned_pool::PinnedPool;
    ///
    /// let mut pool = PinnedPool::<String>::new();
    ///
    /// assert_eq!(pool.len(), 0);
    /// assert!(pool.is_empty());
    ///
    /// let key = pool.insert("Hello".to_string());
    /// assert_eq!(pool.len(), 1);
    /// assert!(!pool.is_empty());
    ///
    /// let item = pool.get(key);
    /// assert_eq!(&*item, "Hello");
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if `T` is zero-sized.
    #[must_use]
    pub fn new() -> Self {
        Self::builder().build()
    }

    /// Starts building a new [`PinnedPool`].
    ///
    /// Use this when you want to customize the pool configuration beyond the defaults.
    ///
    /// # Example
    ///
    /// ```rust
    /// use pinned_pool::{DropPolicy, PinnedPool};
    ///
    /// let pool = PinnedPool::<u32>::builder()
    ///     .drop_policy(DropPolicy::MustNotDropItems)
    ///     .build();
    ///
    /// assert_eq!(pool.len(), 0);
    /// assert!(pool.is_empty());
    /// ```
    pub fn builder() -> PinnedPoolBuilder<T> {
        PinnedPoolBuilder::new()
    }

    /// The number of items in the pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use pinned_pool::PinnedPool;
    ///
    /// let mut pool = PinnedPool::<i32>::new();
    /// assert_eq!(pool.len(), 0);
    ///
    /// let key1 = pool.insert(42);
    /// assert_eq!(pool.len(), 1);
    ///
    /// let key2 = pool.insert(24);
    /// assert_eq!(pool.len(), 2);
    ///
    /// pool.remove(key1);
    /// assert_eq!(pool.len(), 1);
    /// # pool.remove(key2);
    /// ```
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // Can be mutated to infinitely growing memory use.
    pub fn len(&self) -> usize {
        self.slabs.iter().map(PinnedSlab::len).sum()
    }

    /// The number of items the pool can accommodate without additional resource allocation.
    ///
    /// This is the total capacity, including any existing items. The capacity may grow
    /// automatically when items are inserted and no space is available.
    ///
    /// # Example
    ///
    /// ```rust
    /// use pinned_pool::PinnedPool;
    ///
    /// let mut pool = PinnedPool::<u8>::new();
    ///
    /// // New pool starts with zero capacity
    /// assert_eq!(pool.capacity(), 0);
    ///
    /// // Inserting items may increase capacity
    /// let key = pool.insert(42);
    /// assert!(pool.capacity() > 0);
    /// assert!(pool.capacity() >= pool.len());
    /// # pool.remove(key);
    /// ```
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.slabs.len()
            .checked_mul(SLAB_CAPACITY)
            .expect("overflow here would mean the pool can hold more items than virtual memory can fit, which makes no sense - it would never grow that big")
    }

    /// Whether the pool is empty.
    ///
    /// An empty pool may still be holding unused capacity.
    ///
    /// # Example
    ///
    /// ```rust
    /// use pinned_pool::PinnedPool;
    ///
    /// let mut pool = PinnedPool::<u16>::new();
    /// assert!(pool.is_empty());
    ///
    /// let key = pool.insert(123);
    /// assert!(!pool.is_empty());
    ///
    /// pool.remove(key);
    /// assert!(pool.is_empty());
    /// ```
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.slabs.iter().all(PinnedSlab::is_empty)
    }

    /// Reserves capacity for at least `additional` more items to be inserted in the pool.
    ///
    /// The pool may reserve more space to speculatively avoid frequent reallocations.
    /// After calling `reserve`, capacity will be greater than or equal to
    /// `self.len() + additional`. Does nothing if capacity is already sufficient.
    ///
    /// # Example
    ///
    /// ```rust
    /// use pinned_pool::PinnedPool;
    ///
    /// let mut pool = PinnedPool::<u32>::new();
    ///
    /// // Reserve space for 10 more items
    /// pool.reserve(10);
    /// assert!(pool.capacity() >= 10);
    ///
    /// // Insert an item - should not need to allocate more capacity
    /// let key = pool.insert(42);
    ///
    /// // Reserve additional space on top of existing items
    /// pool.reserve(5);
    /// assert!(pool.capacity() >= pool.len() + 5);
    /// # pool.remove(key);
    /// ```
    #[cfg_attr(test, mutants::skip)] // Can be mutated to infinitely growing memory use.
    pub fn reserve(&mut self, additional: usize) {
        let required_capacity = self
            .len()
            .checked_add(additional)
            .expect("capacity overflow: requested capacity exceeds maximum possible value");

        if self.capacity() >= required_capacity {
            return;
        }

        // Calculate how many additional slabs we need
        let current_slabs = self.slabs.len();
        let required_slabs = required_capacity.div_ceil(SLAB_CAPACITY);
        let additional_slabs = required_slabs.saturating_sub(current_slabs);

        for _ in 0..additional_slabs {
            self.slabs.push(PinnedSlab::new(self.drop_policy));
        }
    }

    /// Shrinks the pool's memory usage by dropping unused capacity.
    ///
    /// This method reduces the pool's memory footprint by removing unused capacity
    /// where possible. Items currently in the pool are preserved and continue to
    /// maintain their pinning guarantees.
    ///
    /// The pool's capacity may be reduced, but all existing keys remain valid.
    ///
    /// # Example
    ///
    /// ```rust
    /// use pinned_pool::PinnedPool;
    ///
    /// let mut pool = PinnedPool::<u32>::new();
    ///
    /// // Insert some items to create slabs
    /// let key1 = pool.insert(1);
    /// let key2 = pool.insert(2);
    /// let initial_capacity = pool.capacity();
    ///
    /// // Remove all items
    /// pool.remove(key1);
    /// pool.remove(key2);
    ///
    /// // Capacity remains the same until we shrink
    /// assert_eq!(pool.capacity(), initial_capacity);
    ///
    /// // Shrink to fit reduces capacity
    /// pool.shrink_to_fit();
    /// assert!(pool.capacity() <= initial_capacity);
    /// ```
    #[cfg_attr(test, mutants::skip)] // Too annoying to test the vacant index caching.
    pub fn shrink_to_fit(&mut self) {
        // Find the last non-empty slab by scanning from the end
        let new_len = self
            .slabs
            .iter()
            .enumerate()
            .rev()
            .find_map(|(idx, slab)| {
                if !slab.is_empty() {
                    Some(idx.checked_add(1).expect("slab index cannot overflow"))
                } else {
                    None
                }
            })
            .unwrap_or(0);

        // If we're about to remove slabs, we need to invalidate the vacant slot cache
        // since it might point to a slab that will no longer exist
        if new_len < self.slabs.len() {
            self.slab_with_vacant_slot_index = None;
        }

        // Truncate the slabs vector to remove empty slabs from the end
        self.slabs.truncate(new_len);
    }

    /// Gets a pinned reference to an item in the pool by its key.
    ///
    /// The returned reference is pinned, guaranteeing that the item will not be moved
    /// in memory. This enables safe creation of pointers to the item.
    ///
    /// # Example
    ///
    /// ```rust
    /// use pinned_pool::PinnedPool;
    ///
    /// let mut pool = PinnedPool::<String>::new();
    /// let key = pool.insert("Hello, World!".to_string());
    ///
    /// let item = pool.get(key);
    /// assert_eq!(&*item, "Hello, World!");
    ///
    /// // The item is pinned, so we can safely get a pointer to it
    /// let ptr = item.as_ref().get_ref() as *const String;
    /// # pool.remove(key);
    /// ```
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
    /// The returned reference is pinned and mutable, guaranteeing that the item will not
    /// be moved in memory while allowing modification. This enables safe creation of
    /// mutable pointers to the item.
    ///
    /// # Example
    ///
    /// ```rust
    /// use pinned_pool::PinnedPool;
    ///
    /// let mut pool = PinnedPool::<String>::new();
    /// let key = pool.insert("Hello".to_string());
    ///
    /// // Get a mutable reference and modify the item
    /// let mut item = pool.get_mut(key);
    /// item.as_mut().get_mut().push_str(", World!");
    ///
    /// // Verify the modification
    /// let item = pool.get(key);
    /// assert_eq!(&*item, "Hello, World!");
    /// # pool.remove(key);
    /// ```
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
    /// Using an inserter allows you to obtain the key before the item is inserted and
    /// immediately obtain a pinned reference to the item. This can be more efficient than
    /// [`insert()`] when you need immediate access to the inserted item.
    ///
    /// # Example
    ///
    /// ```rust
    /// use pinned_pool::PinnedPool;
    ///
    /// let mut pool = PinnedPool::<String>::new();
    ///
    /// // Get the key before insertion
    /// let inserter = pool.begin_insert();
    /// let key = inserter.key();
    ///
    /// // Insert and get immediate access to the item
    /// let item = inserter.insert("Hello".to_string());
    /// assert_eq!(&*item, "Hello");
    ///
    /// // The key can be used for later access
    /// let same_item = pool.get(key);
    /// assert_eq!(&*same_item, "Hello");
    /// # pool.remove(key);
    /// ```
    ///
    /// For example, using an inserter allows you to obtain the key before the item is inserted
    /// and allows you to immediately obtain a pinned reference to the item.
    ///
    /// [`insert()`]: Self::insert
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
    ///
    /// The item is guaranteed to remain pinned in memory until it is removed from the pool.
    /// The returned key can be used to access the item via [`get()`] or [`get_mut()`].
    ///
    /// # Example
    ///
    /// ```rust
    /// use pinned_pool::PinnedPool;
    ///
    /// let mut pool = PinnedPool::<i32>::new();
    ///
    /// let key = pool.insert(42);
    /// let item = pool.get(key);
    /// assert_eq!(*item, 42);
    ///
    /// // Keys can be stored and used later
    /// let another_key = pool.insert(24);
    /// assert_eq!(*pool.get(another_key), 24);
    /// # pool.remove(key);
    /// # pool.remove(another_key);
    /// ```
    ///
    /// [`get()`]: Self::get
    /// [`get_mut()`]: Self::get_mut
    #[must_use]
    pub fn insert(&mut self, value: T) -> Key {
        let inserter = self.begin_insert();
        let key = inserter.key();
        inserter.insert(value);
        key
    }

    /// Removes an item from the pool by its key.
    ///
    /// After an item is removed, any pointers to it become invalid and must not be used.
    /// The key may be reused for future insertions.
    ///
    /// # Example
    ///
    /// ```rust
    /// use pinned_pool::PinnedPool;
    ///
    /// let mut pool = PinnedPool::<String>::new();
    /// let key = pool.insert("Hello".to_string());
    ///
    /// assert_eq!(pool.len(), 1);
    /// assert!(!pool.is_empty());
    ///
    /// pool.remove(key);
    ///
    /// assert_eq!(pool.len(), 0);
    /// assert!(pool.is_empty());
    /// ```
    ///
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
        self.update_vacant_slot_cache(index.slab_index);
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

/// An inserter for a [`PinnedPool`], enabling advanced item insertion scenarios.
///
/// The inserter allows you to:
/// - Obtain the key before inserting the item via [`key()`]
/// - Insert an item and get immediate access via [`insert()`] or [`insert_mut()`]
/// - Avoid separate lookup operations when immediate access is needed
///
/// Created by calling [`PinnedPool::begin_insert()`].
///
/// # Example
///
/// ```rust
/// use pinned_pool::PinnedPool;
///
/// let mut pool = PinnedPool::<String>::new();
///
/// // Create an inserter
/// let inserter = pool.begin_insert();
///
/// // Get the key that will be assigned
/// let key = inserter.key();
///
/// // Insert and get immediate mutable access
/// let mut item = inserter.insert_mut("Hello".to_string());
/// item.as_mut().get_mut().push_str(", World!");
///
/// // The item can also be accessed later via the key
/// let same_item = pool.get(key);
/// assert_eq!(&*same_item, "Hello, World!");
/// # pool.remove(key);
/// ```
///
/// [`key()`]: Self::key
/// [`insert()`]: Self::insert
/// [`insert_mut()`]: Self::insert_mut
/// [`PinnedPool::begin_insert()`]: PinnedPool::begin_insert
///
/// [1]: PinnedPool::insert
#[derive(Debug)]
pub struct PinnedPoolInserter<'s, T> {
    slab_inserter: PinnedSlabInserter<'s, T, SLAB_CAPACITY>,
    slab_index: usize,
}

impl<'s, T> PinnedPoolInserter<'s, T> {
    /// Inserts an item and returns a pinned reference to it.
    ///
    /// This provides immediate access to the inserted item without requiring a separate lookup.
    /// The item is guaranteed to remain pinned in memory until removed from the pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use pinned_pool::PinnedPool;
    ///
    /// let mut pool = PinnedPool::<String>::new();
    /// let inserter = pool.begin_insert();
    /// let key = inserter.key();
    ///
    /// let item = inserter.insert("Hello, World!".to_string());
    /// assert_eq!(&*item, "Hello, World!");
    ///
    /// // The item can also be accessed later via the key
    /// let same_item = pool.get(key);
    /// assert_eq!(&*same_item, "Hello, World!");
    /// # pool.remove(key);
    /// ```
    pub fn insert<'v>(self, value: T) -> Pin<&'v T>
    where
        's: 'v,
    {
        self.slab_inserter.insert(value)
    }

    /// Inserts an item and returns a pinned exclusive reference to it.
    ///
    /// This provides immediate mutable access to the inserted item without requiring a separate lookup.
    /// The item is guaranteed to remain pinned in memory until removed from the pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use pinned_pool::PinnedPool;
    ///
    /// let mut pool = PinnedPool::<String>::new();
    /// let inserter = pool.begin_insert();
    /// let key = inserter.key();
    ///
    /// let mut item = inserter.insert_mut("Hello".to_string());
    /// item.as_mut().get_mut().push_str(", World!");
    ///
    /// // Verify the modification
    /// let item = pool.get(key);
    /// assert_eq!(&*item, "Hello, World!");
    /// # pool.remove(key);
    /// ```
    pub fn insert_mut<'v>(self, value: T) -> Pin<&'v mut T>
    where
        's: 'v,
    {
        self.slab_inserter.insert_mut(value)
    }

    /// The key of the item that will be inserted by this inserter.
    ///
    /// This allows you to obtain the key before actually inserting the item, which can be
    /// useful when the item needs to know its own key during construction or initialization.
    ///
    /// # Example
    ///
    /// ```rust
    /// use pinned_pool::PinnedPool;
    ///
    /// let mut pool = PinnedPool::<String>::new();
    /// let inserter = pool.begin_insert();
    ///
    /// // Get the key before insertion
    /// let key = inserter.key();
    ///
    /// // Use the key to create the item (useful for self-referential data)
    /// let item_content = format!("Item with key: {:?}", key);
    /// let item = inserter.insert(item_content);
    ///
    /// // Verify the item was inserted correctly
    /// assert!(item.contains("Item with key:"));
    /// # pool.remove(key);
    /// ```
    ///
    /// If the inserter is abandoned, the key may be used by a different item inserted later.
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
    #![allow(
        clippy::indexing_slicing,
        clippy::cast_possible_truncation,
        reason = "we do not need to worry about these things when writing test code"
    )]

    use std::cell::RefCell;
    use std::sync::{Arc, Mutex};
    use std::{ptr, thread};

    use super::*;

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

    #[test]
    fn fill_first_slab_before_allocating_second() {
        let mut pool = PinnedPool::<u32>::new();

        for _ in 0..SLAB_CAPACITY {
            _ = pool.insert(1234);
        }

        assert_eq!(pool.slabs.len(), 1);
        assert!(pool.slabs[0].is_full());

        // This will allocate a second slab.
        _ = pool.insert(1234);

        assert_eq!(pool.slabs.len(), 2);
    }

    #[test]
    fn fill_first_slab_even_after_abandoned_insert() {
        let mut pool = PinnedPool::<u32>::new();

        // Leave space for 1 item.
        for _ in 0..(SLAB_CAPACITY - 1) {
            _ = pool.insert(1234);
        }

        assert_eq!(pool.slabs.len(), 1);
        assert!(!pool.slabs[0].is_full());

        // Begin an insert but do not complete it.
        _ = pool.begin_insert();

        // Ensure that the next inserted item still goes into the first slab.
        // That is, we did not "waste" the vacant slot in the first slab
        // due to the abandoned insert.
        _ = pool.insert(1234);

        assert_eq!(pool.slabs.len(), 1);
        assert!(pool.slabs[0].is_full());
    }

    #[test]
    fn fill_hole_before_allocating_new_slab() {
        let mut pool = PinnedPool::<u32>::new();

        // Fill the first slab.
        for _ in 0..SLAB_CAPACITY {
            _ = pool.insert(1234);
        }

        // Remove the first item to create a hole.
        let key_to_remove = Key { index_in_pool: 0 };
        pool.remove(key_to_remove);

        // This will fill the hole instead of allocating a new slab.
        let key_filled = pool.insert(5678);

        assert_eq!(key_filled.index_in_pool, 0);
        assert_eq!(*pool.get(key_filled), 5678);
    }

    #[test]
    fn fill_first_hole_ascending() {
        // If two slabs have a hole, we always fill a hole in the first (index-wise) slab.
        // We do not care which hole we fill (there may be multiple per slab), we just care
        // about which slab it is in.
        //
        // We create the holes in ascending order (first slab first, then second slab).

        let mut pool = PinnedPool::<u32>::new();

        // Fill the first slab.
        for _ in 0..SLAB_CAPACITY {
            _ = pool.insert(1234);
        }

        // Fill the second slab.
        for _ in 0..SLAB_CAPACITY {
            _ = pool.insert(5678);
        }

        // Remove the first item in the first slab to create a hole.
        let key_to_remove = Key { index_in_pool: 0 };
        pool.remove(key_to_remove);

        // Remove the first item in the second slab to create a hole.
        let key_to_remove = Key {
            index_in_pool: SLAB_CAPACITY,
        };
        pool.remove(key_to_remove);

        // This will fill the hole in the first slab instead of allocating a new slab.
        let key_filled = pool.insert(91011);

        assert_eq!(key_filled.index_in_pool, 0);
        assert_eq!(*pool.get(key_filled), 91011);
    }

    #[test]
    fn fill_first_hole_descending() {
        // If two slabs have a hole, we always fill a hole in the first (index-wise) slab.
        // We do not care which hole we fill (there may be multiple per slab), we just care
        // about which slab it is in.
        //
        // We create the holes in descending order (second slab first, then first slab).

        let mut pool = PinnedPool::<u32>::new();

        // Fill the first slab.
        for _ in 0..SLAB_CAPACITY {
            _ = pool.insert(1234);
        }

        // Fill the second slab.
        for _ in 0..SLAB_CAPACITY {
            _ = pool.insert(5678);
        }

        // Remove the first item in the second slab to create a hole.
        let key_to_remove = Key {
            index_in_pool: SLAB_CAPACITY,
        };
        pool.remove(key_to_remove);

        // Remove the first item in the first slab to create a hole.
        let key_to_remove = Key { index_in_pool: 0 };
        pool.remove(key_to_remove);

        // This will fill the hole in the first slab instead of allocating a new slab.
        let key_filled = pool.insert(91011);

        assert_eq!(key_filled.index_in_pool, 0);
        assert_eq!(*pool.get(key_filled), 91011);
    }

    #[test]
    #[should_panic]
    fn zst_is_panic() {
        drop(PinnedPool::<()>::new());
    }

    #[test]
    fn insert_mut_then_get_is_correct_value() {
        let mut pool = PinnedPool::<u32>::new();

        let inserter = pool.begin_insert();
        let key = inserter.key();
        let mut item = inserter.insert_mut(42);
        *item = 99;

        assert_eq!(*pool.get(key), 99);
    }

    #[test]
    fn default_works_fine() {
        let mut pool: PinnedPool<u32> = PinnedPool::default();
        assert!(pool.is_empty());
        assert_eq!(pool.len(), 0);
        assert_eq!(pool.capacity(), 0);

        let key = pool.insert(1234);
        assert!(!pool.is_empty());
        assert_eq!(pool.len(), 1);

        assert_eq!(pool.get(key).get_ref(), &1234);

        pool.remove(key);
    }

    #[test]
    fn shrink_to_fit_removes_empty_slabs() {
        let mut pool = PinnedPool::<u32>::new();

        // Insert enough items to create multiple slabs
        let mut keys = Vec::new();
        for i in 0..(SLAB_CAPACITY * 3) {
            keys.push(pool.insert(i as u32));
        }

        // Verify we have 3 slabs
        assert_eq!(pool.capacity(), SLAB_CAPACITY * 3);

        // Remove all items from the last two slabs, keeping the first slab full
        for key in keys.iter().skip(SLAB_CAPACITY) {
            pool.remove(*key);
        }

        // Capacity should still be 3 slabs
        assert_eq!(pool.capacity(), SLAB_CAPACITY * 3);

        // Shrink to fit should remove the empty slabs
        pool.shrink_to_fit();

        // Now capacity should be 1 slab
        assert_eq!(pool.capacity(), SLAB_CAPACITY);

        // Verify the remaining items are still accessible
        for (i, key) in keys.iter().take(SLAB_CAPACITY).enumerate() {
            assert_eq!(*pool.get(*key), i as u32);
        }
    }

    #[test]
    fn shrink_to_fit_all_empty_slabs() {
        let mut pool = PinnedPool::<u32>::new();

        // Insert items to create slabs
        let mut keys = Vec::new();
        for i in 0..(SLAB_CAPACITY * 2) {
            keys.push(pool.insert(i as u32));
        }

        // Verify we have 2 slabs
        assert_eq!(pool.capacity(), SLAB_CAPACITY * 2);

        // Remove all items
        for key in keys {
            pool.remove(key);
        }

        // Capacity should still be 2 slabs
        assert_eq!(pool.capacity(), SLAB_CAPACITY * 2);

        // Shrink to fit should remove all slabs
        pool.shrink_to_fit();

        // Now capacity should be 0
        assert_eq!(pool.capacity(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn shrink_to_fit_no_empty_slabs() {
        let mut pool = PinnedPool::<u32>::new();

        // Insert items to fill slabs completely
        let mut keys = Vec::new();
        for i in 0..(SLAB_CAPACITY * 2) {
            keys.push(pool.insert(i as u32));
        }

        let original_capacity = pool.capacity();

        // Shrink to fit should not change anything since no slabs are empty
        pool.shrink_to_fit();

        assert_eq!(pool.capacity(), original_capacity);

        // Verify all items are still accessible
        for (i, key) in keys.iter().enumerate() {
            assert_eq!(*pool.get(*key), i as u32);
        }
    }

    #[test]
    fn shrink_to_fit_empty_pool() {
        let mut pool = PinnedPool::<u32>::new();

        // Pool starts empty
        assert_eq!(pool.capacity(), 0);

        // Shrink to fit should not change anything
        pool.shrink_to_fit();

        assert_eq!(pool.capacity(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn shrink_then_grow_allocates_new_slab() {
        let mut pool = PinnedPool::<u32>::new();

        // Fill one complete slab
        let mut keys = Vec::new();
        for i in 0..SLAB_CAPACITY {
            keys.push(pool.insert(i as u32));
        }

        // Add one item to the second slab
        let overflow_key = pool.insert(9999_u32);

        // Verify we have 2 slabs
        assert_eq!(pool.slabs.len(), 2);
        assert_eq!(pool.capacity(), SLAB_CAPACITY * 2);

        // Remove the overflow item (making the second slab empty)
        pool.remove(overflow_key);

        // Shrink to fit should remove the empty second slab
        pool.shrink_to_fit();

        // Verify we're back to 1 slab
        assert_eq!(pool.slabs.len(), 1);
        assert_eq!(pool.capacity(), SLAB_CAPACITY);
        assert!(pool.slabs[0].is_full());

        // Insert a new item - this should allocate a new slab since the existing one is full
        let new_key = pool.insert(8888_u32);

        // Verify we now have 2 slabs again
        assert_eq!(pool.slabs.len(), 2);
        assert_eq!(pool.capacity(), SLAB_CAPACITY * 2);

        // Verify the new item went to the second slab
        assert_eq!(new_key.index_in_pool, SLAB_CAPACITY);

        // Verify the new item is accessible
        assert_eq!(*pool.get(new_key), 8888);

        // Clean up
        for key in keys {
            pool.remove(key);
        }
        pool.remove(new_key);
    }

    #[test]
    fn reserve_increases_capacity() {
        let mut pool = PinnedPool::<u32>::new();

        // Initially no capacity
        assert_eq!(pool.capacity(), 0);

        // Reserve space for 10 items
        pool.reserve(10);
        assert!(pool.capacity() >= 10);

        // Insert an item - should not need to allocate more capacity
        let initial_capacity = pool.capacity();
        let key = pool.insert(42);
        assert_eq!(pool.capacity(), initial_capacity);

        pool.remove(key);
    }

    #[test]
    fn reserve_with_existing_items() {
        let mut pool = PinnedPool::<u32>::new();

        // Insert some items first
        let key1 = pool.insert(1);
        let key2 = pool.insert(2);
        let current_len = pool.len();

        // Reserve additional space
        pool.reserve(5);
        assert!(pool.capacity() >= current_len + 5);

        // Verify existing items are still accessible
        assert_eq!(*pool.get(key1), 1);
        assert_eq!(*pool.get(key2), 2);

        pool.remove(key1);
        pool.remove(key2);
    }

    #[test]
    fn reserve_zero_does_nothing() {
        let mut pool = PinnedPool::<u32>::new();
        let initial_capacity = pool.capacity();

        pool.reserve(0);
        assert_eq!(pool.capacity(), initial_capacity);
    }

    #[test]
    fn reserve_with_sufficient_capacity_does_nothing() {
        let mut pool = PinnedPool::<u32>::new();

        // Reserve initial capacity
        pool.reserve(10);
        let capacity_after_reserve = pool.capacity();

        // Try to reserve less than what we already have
        pool.reserve(5);
        assert_eq!(pool.capacity(), capacity_after_reserve);
    }

    #[test]
    fn reserve_large_capacity() {
        let mut pool = PinnedPool::<u32>::new();

        // Reserve capacity for multiple slabs
        let large_count = SLAB_CAPACITY * 3 + 50;
        pool.reserve(large_count);
        assert!(pool.capacity() >= large_count);

        // Verify we can actually insert that many items
        let mut keys = Vec::new();
        for i in 0..large_count {
            keys.push(pool.insert(i as u32));
        }

        // Verify all items are accessible
        for (i, &key) in keys.iter().enumerate() {
            assert_eq!(*pool.get(key), i as u32);
        }

        // Clean up
        for key in keys {
            pool.remove(key);
        }
    }

    #[test]
    #[should_panic(expected = "capacity overflow")]
    fn reserve_overflow_panics() {
        let mut pool = PinnedPool::<u32>::new();

        // Insert one item to make len() = 1
        let _key = pool.insert(42);

        // Try to reserve usize::MAX more items. Since len() = 1,
        // this will cause 1 + usize::MAX to overflow during capacity calculation
        pool.reserve(usize::MAX);
    }

    #[test]
    fn trait_object_usage() {
        // Define a simple trait for testing.
        trait Greet {
            fn greet(&self) -> String;
        }

        // Implement the trait for a concrete type.
        #[derive(Debug)]
        struct Person {
            name: String,
        }

        impl Greet for Person {
            fn greet(&self) -> String {
                format!("Hello, I'm {}", self.name)
            }
        }

        let mut pool = PinnedPool::<Person>::new();

        // Insert concrete type into the pool.
        let person_key = pool.insert(Person {
            name: "Alice".to_string(),
        });

        // Access item and convert to trait object.
        let person_ref = pool.get(person_key);
        let greet_obj: &dyn Greet = person_ref.get_ref();

        // Use the trait method on the trait object.
        assert_eq!(greet_obj.greet(), "Hello, I'm Alice");

        // Clean up.
        pool.remove(person_key);
    }

    #[test]
    fn trait_object_with_pinned_references() {
        trait Identifiable {
            fn get_id(&self) -> u64;
            fn set_id(&mut self, id: u64);
        }

        #[derive(Debug)]
        struct Item {
            id: u64,
            #[expect(dead_code, reason = "Used for demo purposes")]
            data: String,
        }

        impl Identifiable for Item {
            fn get_id(&self) -> u64 {
                self.id
            }

            fn set_id(&mut self, id: u64) {
                self.id = id;
            }
        }

        let mut pool = PinnedPool::<Item>::new();

        let item_key = pool.insert(Item {
            id: 123,
            data: "test data".to_string(),
        });

        // Get a pinned reference and use it as a trait object.
        {
            let item_ref = pool.get(item_key);
            let trait_obj: &dyn Identifiable = item_ref.get_ref();
            assert_eq!(trait_obj.get_id(), 123);
        }

        // Get a mutable pinned reference and use it as a trait object.
        {
            let item_ref = pool.get_mut(item_key);
            let trait_obj: &mut dyn Identifiable = item_ref.get_mut();
            trait_obj.set_id(456);
            assert_eq!(trait_obj.get_id(), 456);
        }

        // Verify the change persisted.
        {
            let item_ref = pool.get(item_key);
            assert_eq!(item_ref.id, 456);
        }

        pool.remove(item_key);
    }
}
