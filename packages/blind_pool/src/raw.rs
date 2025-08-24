use std::alloc::Layout;
use std::mem::MaybeUninit;
use std::ptr::NonNull;
use std::{fmt, thread};

use foldhash::{HashMap, HashMapExt};
use opaque_pool::{DropPolicy, OpaquePool, Pooled as OpaquePooled, PooledMut};

use crate::RawBlindPoolBuilder;

/// A pinned object pool of unbounded size that accepts objects of any type.
///
/// The pool returns a `RawPooled<T>` for each inserted value, which acts as a super-powered
/// pointer that can be copied and cloned freely. Each handle provides direct access to the
/// inserted item via a pointer.
///
/// # Out of band access
///
/// The collection does not create or keep references to the memory blocks. The only way to access
/// the contents of the collection is via unsafe code by using the pointer from a `RawPooled<T>`.
///
/// The collection does not create or maintain any `&` shared or `&mut` exclusive references to
/// the items it contains, except when explicitly called to operate on an item (e.g. `remove()`
/// implies exclusive access).
///
/// # Resource usage
///
/// The collection automatically grows as items are added. To reduce memory usage after items have
/// been removed, use the [`shrink_to_fit()`][1] method to release unused capacity.
///
/// [1]: Self::shrink_to_fit
///
/// # Example
///
/// ```rust
/// use blind_pool::RawBlindPool;
///
/// let mut pool = RawBlindPool::new();
///
/// // Insert values of different types.
/// let pooled_string1 = pool.insert("Hello".to_string());
/// let pooled_string2 = pool.insert("World".to_string());
///
/// // Read from the memory.
/// // SAFETY: The pointers are valid and the memory contains the values we just inserted.
/// let value_string1 = unsafe { pooled_string1.ptr().as_ref() };
/// let value_string2 = unsafe { pooled_string2.ptr().as_ref() };
///
/// assert_eq!(value_string1, "Hello");
/// assert_eq!(value_string2, "World");
/// ```
///
/// # Thread safety
///
/// This type is thread-mobile ([`Send`]) but not thread-safe ([`Sync`]). It can be moved
/// between threads but cannot be shared between threads simultaneously. For thread-safe
/// pool operations, use [`BlindPool`][crate::BlindPool] instead.
pub struct RawBlindPool {
    /// Internal pools, one for each unique memory layout encountered.
    /// We use foldhash for better performance with small hash tables.
    pools: HashMap<Layout, OpaquePool>,

    /// Drop policy that determines how the pool handles remaining items when dropped.
    drop_policy: DropPolicy,
}

impl RawBlindPool {
    /// Creates a new `RawBlindPool` with default configuration.
    ///
    /// For custom configuration, use [`RawBlindPool::builder()`][RawBlindPool::builder].
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    ///
    /// let pooled = pool.insert("Test".to_string());
    ///
    /// // SAFETY: The pointer is valid and contains the value we just inserted.
    /// let value = unsafe { pooled.ptr().as_ref() };
    /// assert_eq!(value.as_str(), "Test");
    /// ```
    #[must_use]
    pub fn new() -> Self {
        Self::builder().build()
    }

    /// Creates a builder for configuring and constructing a [`RawBlindPool`].
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::{DropPolicy, RawBlindPool};
    ///
    /// let pool = RawBlindPool::builder()
    ///     .drop_policy(DropPolicy::MustNotDropItems)
    ///     .build();
    /// ```
    pub fn builder() -> RawBlindPoolBuilder {
        RawBlindPoolBuilder::new()
    }

    /// Creates a new `RawBlindPool` with the specified configuration.
    ///
    /// This method is used internally by the builder to construct the actual pool.
    #[must_use]
    pub(crate) fn new_inner(drop_policy: DropPolicy) -> Self {
        Self {
            pools: HashMap::new(),
            drop_policy,
        }
    }

    /// Inserts a value into the pool and returns a handle to access it.
    ///
    /// The pool stores the value and provides a handle for later access or removal.
    ///
    /// The caller must ensure that if `T` contains any references or other lifetime-dependent
    /// data, those lifetimes are valid for the entire duration that the value may remain in
    /// the pool. Since access to pool contents is only possible through unsafe code, the caller
    /// is responsible for ensuring that no use-after-free conditions occur.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    ///
    /// // Insert different types into the same pool.
    /// let _pooled_string = pool.insert("Hello".to_string());
    /// let _pooled_float = pool.insert(2.5_f64);
    /// let _pooled_string = pool.insert("hello".to_string());
    ///
    /// // All values are stored in the same BlindPool.
    /// assert_eq!(pool.len(), 3);
    /// ```
    #[inline]
    pub fn insert<T>(&mut self, value: T) -> RawPooled<T> {
        let layout = Layout::new::<T>();

        let internal_pool = self.pools.entry(layout).or_insert_with(|| {
            OpaquePool::builder()
                .layout_of::<T>()
                .drop_policy(self.drop_policy)
                .build()
        });

        // SAFETY: The internal pool was created with the same layout as T, ensuring
        // that T's size and alignment requirements are satisfied by the pool's allocation strategy.
        let pooled = unsafe { internal_pool.insert(value) }.into_shared();

        RawPooled {
            layout,
            inner: pooled,
        }
    }

    /// Inserts a value into the pool using in-place initialization and returns a handle to it.
    ///
    /// This method is designed for partial object initialization, where you want to construct
    /// an object directly in its final memory location. This can provide significant
    /// performance benefits compared to [`insert()`] by avoiding temporary allocations
    /// and unnecessary moves, especially for large or complex types.
    ///
    /// The pool stores the initialized value and provides a handle for later access or removal.
    ///
    /// [`insert()`]: Self::insert
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::mem::MaybeUninit;
    ///
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    ///
    /// // Partial initialization - build complex object directly in pool memory.
    /// // SAFETY: We properly initialize the value in the closure.
    /// let pooled = unsafe {
    ///     pool.insert_with(|uninit: &mut MaybeUninit<Vec<u64>>| {
    ///         let mut vec = Vec::with_capacity(1000);
    ///         vec.extend(0..100);
    ///         uninit.write(vec);
    ///     })
    /// };
    ///
    /// // Read the value back.
    /// // SAFETY: The pointer is valid and contains the initialized Vec.
    /// let value = unsafe { pooled.ptr().as_ref() };
    /// assert_eq!(value.len(), 100);
    ///
    /// // Clean up.
    /// unsafe { pool.remove(&pooled) };
    /// ```
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    /// - The closure properly initializes the `MaybeUninit<T>` before returning.
    /// - If `T` contains any references or other lifetime-dependent data, those lifetimes
    ///   are valid for the entire duration that the value may remain in the pool. Since
    ///   access to pool contents is only possible through unsafe code, the caller is
    ///   responsible for ensuring that no use-after-free conditions occur.
    #[inline]
    pub unsafe fn insert_with<T>(&mut self, f: impl FnOnce(&mut MaybeUninit<T>)) -> RawPooled<T> {
        let layout = Layout::new::<T>();

        let internal_pool = self.pools.entry(layout).or_insert_with(|| {
            OpaquePool::builder()
                .layout_of::<T>()
                .drop_policy(self.drop_policy)
                .build()
        });

        // SAFETY: The internal pool was created with the same layout as T, ensuring
        // that T's size and alignment requirements are satisfied by the pool's allocation strategy.
        // We forward the safety requirements to the caller.
        let pooled = unsafe { internal_pool.insert_with(f) }.into_shared();

        RawPooled {
            layout,
            inner: pooled,
        }
    }

    /// Inserts a value into the pool and returns an exclusive handle to it.
    ///
    /// Unlike [`insert()`], this method returns a [`RawPooledMut<T>`] that provides exclusive
    /// ownership guarantees. The handle cannot be copied or cloned, ensuring that only one
    /// handle can exist for each pool item. This enables safe removal methods that consume
    /// the handle, preventing double-use bugs.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    ///
    /// // Insert with exclusive ownership.
    /// let pooled_mut = pool.insert_mut("Test".to_string());
    ///
    /// // Safe removal that consumes the handle.
    /// let extracted = pool.remove_unpin_mut(pooled_mut);
    /// assert_eq!(extracted, "Test");
    /// ```
    ///
    /// [`insert()`]: Self::insert
    #[inline]
    pub fn insert_mut<T>(&mut self, value: T) -> RawPooledMut<T> {
        let layout = Layout::new::<T>();

        let internal_pool = self.pools.entry(layout).or_insert_with(|| {
            OpaquePool::builder()
                .layout_of::<T>()
                .drop_policy(self.drop_policy)
                .build()
        });

        // SAFETY: The internal pool was created with the same layout as T, ensuring
        // that T's size and alignment requirements are satisfied by the pool's allocation strategy.
        let pooled_mut = unsafe { internal_pool.insert(value) };

        RawPooledMut {
            layout,
            inner: pooled_mut,
        }
    }

    /// Inserts a value into the pool using in-place initialization and returns an exclusive handle.
    ///
    /// This method is designed for partial object initialization with exclusive ownership.
    /// It returns a [`RawPooledMut<T>`] that provides exclusive ownership guarantees.
    /// The handle cannot be copied or cloned, ensuring that only one handle can exist
    /// for each pool item.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::mem::MaybeUninit;
    ///
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    ///
    /// // Partial initialization with exclusive ownership.
    /// // SAFETY: We properly initialize the value in the closure.
    /// let pooled_mut = unsafe {
    ///     pool.insert_with_mut(|uninit: &mut MaybeUninit<Vec<u64>>| {
    ///         let mut vec = Vec::with_capacity(1000);
    ///         vec.extend(0..100);
    ///         uninit.write(vec);
    ///     })
    /// };
    ///
    /// // Safe removal that consumes the handle.
    /// let extracted = pool.remove_unpin_mut(pooled_mut);
    /// assert_eq!(extracted.len(), 100);
    /// ```
    ///
    /// # Safety
    ///
    /// The caller must ensure that:
    /// - The closure properly initializes the `MaybeUninit<T>` before returning.
    /// - If `T` contains any references or other lifetime-dependent data, those lifetimes
    ///   are valid for the entire duration that the value may remain in the pool.
    ///
    /// [`insert_with()`]: Self::insert_with
    #[inline]
    pub unsafe fn insert_with_mut<T>(
        &mut self,
        f: impl FnOnce(&mut MaybeUninit<T>),
    ) -> RawPooledMut<T> {
        let layout = Layout::new::<T>();

        let internal_pool = self.pools.entry(layout).or_insert_with(|| {
            OpaquePool::builder()
                .layout_of::<T>()
                .drop_policy(self.drop_policy)
                .build()
        });

        // SAFETY: The internal pool was created with the same layout as T, ensuring
        // that T's size and alignment requirements are satisfied by the pool's allocation strategy.
        // We forward the safety requirements to the caller.
        let pooled_mut = unsafe { internal_pool.insert_with(f) };

        RawPooledMut {
            layout,
            inner: pooled_mut,
        }
    }

    /// Removes a value from the pool and drops it.
    ///
    /// The `RawPooled<T>` handle is consumed and the memory is returned to the pool.
    /// The value is dropped.
    ///
    /// # Panics
    ///
    /// Panics if the provided handle does not belong to this pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    ///
    /// let _pooled = pool.insert("Test".to_string());
    /// assert_eq!(pool.len(), 1);
    ///
    /// unsafe { pool.remove(&_pooled) };
    /// assert_eq!(pool.len(), 0);
    /// ```
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the pooled handle has not been used for removal before.
    /// Using the same pooled handle multiple times may result in undefined behavior.
    #[inline]
    pub unsafe fn remove<T: ?Sized>(&mut self, pooled: &RawPooled<T>) {
        if let Some(internal_pool) = self.pools.get_mut(&pooled.layout) {
            // SAFETY: The caller guarantees that this pooled handle has not been used before.
            unsafe {
                internal_pool.remove(&pooled.inner);
            }
        } else {
            panic!("provided handle does not belong to this pool");
        }
    }

    /// Removes a value from the pool and returns it.
    ///
    /// The `RawPooled<T>` handle is consumed and the memory is returned to the pool.
    /// The value is moved out and returned to the caller.
    ///
    /// # Panics
    ///
    /// Panics if the provided handle does not belong to this pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    ///
    /// let _pooled = pool.insert("Test".to_string());
    /// assert_eq!(pool.len(), 1);
    ///
    /// let extracted = unsafe { pool.remove_unpin(&_pooled) };
    /// assert_eq!(extracted, "Test");
    /// assert_eq!(pool.len(), 0);
    /// ```
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the pooled handle has not been used for removal before.
    #[inline]
    pub unsafe fn remove_unpin<T: Unpin>(&mut self, pooled: &RawPooled<T>) -> T {
        self.pools.get_mut(&pooled.layout).map_or_else(
            || panic!("provided handle does not belong to this pool"),
            // SAFETY: The caller guarantees that this pooled handle has not been used before.
            |internal_pool| unsafe { internal_pool.remove_unpin(&pooled.inner) },
        )
    }

    /// Removes a value from the pool using an exclusive handle and drops it.
    ///
    /// The `RawPooledMut<T>` handle is consumed and the memory is returned to the pool.
    /// The value is dropped. This is safe because the exclusive handle guarantees that
    /// it cannot be used multiple times.
    ///
    /// # Panics
    ///
    /// Panics if the provided handle does not belong to this pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    ///
    /// let pooled_mut = pool.insert_mut("Test".to_string());
    /// assert_eq!(pool.len(), 1);
    ///
    /// pool.remove_mut(pooled_mut); // Safe - no double-use possible
    /// assert_eq!(pool.len(), 0);
    /// ```
    #[inline]
    pub fn remove_mut<T: ?Sized>(&mut self, pooled_mut: RawPooledMut<T>) {
        if let Some(internal_pool) = self.pools.get_mut(&pooled_mut.layout) {
            internal_pool.remove_mut(pooled_mut.inner);
        } else {
            panic!("provided handle does not belong to this pool");
        }
    }

    /// Removes a value from the pool using an exclusive handle and returns it.
    ///
    /// The `RawPooledMut<T>` handle is consumed and the memory is returned to the pool.
    /// The value is moved out and returned to the caller. This is safe because the
    /// exclusive handle guarantees that it cannot be used multiple times.
    ///
    /// # Panics
    ///
    /// Panics if the provided handle does not belong to this pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    ///
    /// let pooled_mut = pool.insert_mut("Test".to_string());
    /// assert_eq!(pool.len(), 1);
    ///
    /// let extracted = pool.remove_unpin_mut(pooled_mut); // Safe - no double-use possible
    /// assert_eq!(extracted, "Test");
    /// assert_eq!(pool.len(), 0);
    /// ```
    #[inline]
    pub fn remove_unpin_mut<T: Unpin>(&mut self, pooled_mut: RawPooledMut<T>) -> T {
        self.pools.get_mut(&pooled_mut.layout).map_or_else(
            || panic!("provided handle does not belong to this pool"),
            |internal_pool| internal_pool.remove_unpin_mut(pooled_mut.inner),
        )
    }

    /// Returns the total number of items stored in the pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    ///
    /// assert_eq!(pool.len(), 0);
    ///
    /// let _a = pool.insert("Hello".to_string());
    /// let _b = pool.insert("world".to_string());
    /// let _c = pool.insert(2.5_f64);
    ///
    /// assert_eq!(pool.len(), 3);
    /// ```
    #[must_use]
    #[inline]
    pub fn len(&self) -> usize {
        self.pools.values().map(OpaquePool::len).sum()
    }

    /// Whether the pool has no inserted values.
    ///
    /// An empty pool may still be holding unused memory capacity.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    ///
    /// assert!(pool.is_empty());
    ///
    /// let _pooled = pool.insert("Test".to_string());
    ///
    /// assert!(!pool.is_empty());
    ///
    /// unsafe { pool.remove(&_pooled) };
    /// assert!(pool.is_empty());
    /// ```
    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.pools.values().all(OpaquePool::is_empty)
    }

    /// Returns the capacity for items of type `T`.
    ///
    /// This is the number of items of type `T` that can be stored without allocating more memory.
    /// If no items of type `T` have been inserted yet, returns 0.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    ///
    /// // Initially no capacity is allocated for any type.
    /// assert_eq!(pool.capacity_of::<u32>(), 0);
    /// assert_eq!(pool.capacity_of::<f64>(), 0);
    ///
    /// // Inserting a String allocates capacity for String but not f64.
    /// let _pooled = pool.insert("Test".to_string());
    /// assert!(pool.capacity_of::<String>() > 0);
    /// assert_eq!(pool.capacity_of::<f64>(), 0);
    /// ```
    #[must_use]
    #[inline]
    pub fn capacity_of<T>(&self) -> usize {
        let layout = Layout::new::<T>();
        self.pools.get(&layout).map_or(0, OpaquePool::capacity)
    }

    /// Reserves capacity for at least `additional` more items of type `T`.
    ///
    /// The pool may reserve more space to speculatively avoid frequent reallocations.
    /// After calling `reserve_for`, the capacity for type `T` will be greater than or equal to
    /// the current count of `T` items plus `additional`. Does nothing if capacity is already
    /// sufficient.
    ///
    /// If no items of type `T` have been inserted yet, this creates an internal pool for type `T`
    /// and reserves the requested capacity.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    ///
    /// // Reserve space for 10 u32 values specifically
    /// pool.reserve_for::<u32>(10);
    /// assert!(pool.capacity_of::<u32>() >= 10);
    /// assert_eq!(pool.capacity_of::<f64>(), 0); // Other types unaffected
    ///
    /// // Insert String values - should not need to allocate more capacity
    /// let _pooled = pool.insert("Test".to_string());
    /// assert!(pool.capacity_of::<String>() >= 10);
    /// ```
    pub fn reserve_for<T>(&mut self, additional: usize) {
        let layout = Layout::new::<T>();

        let internal_pool = self.pools.entry(layout).or_insert_with(|| {
            OpaquePool::builder()
                .layout_of::<T>()
                .drop_policy(self.drop_policy)
                .build()
        });

        internal_pool.reserve(additional);
    }

    /// Shrinks the capacity of the pool to fit its current size.
    ///
    /// This can help reduce memory usage after items have been removed from the pool.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    ///
    /// // Insert many items to allocate capacity.
    /// for i in 0..1000 {
    ///     pool.insert(i);
    /// }
    ///
    /// // Remove all items but keep the allocated capacity.
    /// while !pool.is_empty() {
    ///     // In a real scenario you'd keep track of handles to remove them properly.
    ///     // This is just for the example.
    ///     break;
    /// }
    ///
    /// // Shrink to fit the current size.
    /// pool.shrink_to_fit();
    /// ```
    pub fn shrink_to_fit(&mut self) {
        // Remove empty internal pools.
        self.pools.retain(|_, pool| !pool.is_empty());

        // Shrink remaining pools.
        for pool in self.pools.values_mut() {
            pool.shrink_to_fit();
        }

        // Shrink the HashMap itself.
        self.pools.shrink_to_fit();
    }
}

impl Default for RawBlindPool {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for RawBlindPool {
    fn drop(&mut self) {
        if matches!(self.drop_policy, DropPolicy::MustNotDropItems) && !thread::panicking() {
            assert!(
                self.is_empty(),
                "BlindPool dropped while still containing items (drop policy is MustNotDropItems)"
            );
        }
    }
}

/// A handle representing an item stored in a `RawBlindPool`.
///
/// Acts as a super-powered pointer that can be copied and cloned freely. This provides
/// access to the stored item and can be used to remove the item from the pool.
///
/// Being `Copy` and `Clone`, this type behaves like a regular pointer - you can duplicate
/// handles freely without affecting the underlying stored value. Multiple copies of the same
/// handle all refer to the same stored value.
///
/// # Example
///
/// ```rust
/// use blind_pool::RawBlindPool;
///
/// let mut pool = RawBlindPool::new();
///
/// let pooled = pool.insert("Hello".to_string());
///
/// // The handle acts like a super-powered pointer - it can be copied freely.
/// let pooled_copy = pooled;
/// let pooled_clone = pooled.clone();
///
/// // All copies refer to the same stored value.
/// // SAFETY: All pointers are valid and point to the same value.
/// let value1 = unsafe { pooled.ptr().as_ref() };
/// let value2 = unsafe { pooled_copy.ptr().as_ref() };
/// let value3 = unsafe { pooled_clone.ptr().as_ref() };
/// assert_eq!(value1, "Hello");
/// assert_eq!(value2, "Hello");
/// assert_eq!(value3, "Hello");
///
/// // To remove the item from the pool, any handle can be used.
/// unsafe { pool.remove(&pooled) };
/// ```
///
/// # Thread safety
///
/// This type is thread-safe ([`Send`] + [`Sync`]) if and only if `T` implements [`Sync`].
/// When `T` is [`Sync`], multiple threads can safely share handles to the same data.
/// When `T` is not [`Sync`], the handle is single-threaded and cannot be moved between threads
/// or shared between threads, preventing unsafe access to non-thread-safe data.
pub struct RawPooled<T: ?Sized> {
    /// The memory layout of the stored item. This is used to identify which internal
    /// pool the item belongs to.
    layout: Layout,

    /// The handle from the internal opaque pool.
    inner: OpaquePooled<T>,
}

impl<T: ?Sized> RawPooled<T> {
    /// Returns a pointer to the inserted value.
    ///
    /// This is the only way to access the value stored in the pool. The owner of the handle has
    /// exclusive access to the value and may both read and write and may create both `&` shared
    /// and `&mut` exclusive references to the item.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    ///
    /// let pooled = pool.insert(2.5159_f64);
    ///
    /// // Read data back from the memory.
    /// // SAFETY: The pointer is valid and the memory contains the value we just inserted.
    /// let value = unsafe { *pooled.ptr().as_ref() };
    /// assert_eq!(value, 2.5159);
    /// ```
    #[must_use]
    #[inline]
    pub fn ptr(&self) -> NonNull<T> {
        self.inner.ptr()
    }

    /// Erases the type information from this `RawPooled<T>` handle,
    /// returning a `RawPooled<()>`.
    ///
    /// This is useful when you want to store handles of different types in the same collection
    /// or pass them to code that doesn't need to know the specific type.
    ///
    /// The handle remains functionally equivalent and can still be used to remove the item
    /// from the pool and drop it. The only change is the removal of the type information.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    ///
    /// let pooled = pool.insert("Test".to_string());
    ///
    /// // Erase type information.
    /// let erased = pooled.erase();
    ///
    /// // Can still access the raw pointer.
    /// // SAFETY: We know this contains a String.
    /// let value = unsafe { erased.ptr().cast::<String>().as_ref() };
    /// assert_eq!(value.as_str(), "Test");
    ///
    /// // Can still remove the item.
    /// unsafe { pool.remove(&erased) };
    /// ```
    #[must_use]
    #[inline]
    pub fn erase(self) -> RawPooled<()> {
        RawPooled {
            layout: self.layout,
            inner: self.inner.erase(),
        }
    }

    /// Casts this `RawPooled<T>` to a different type.
    ///
    /// This method allows casting the pooled value to a trait object or other compatible type.
    /// The underlying memory layout and pool management remain unchanged.
    ///
    /// This method is primarily intended for use by the [`define_pooled_dyn_cast!`] macro.
    /// For most use cases, prefer the type-safe cast methods generated by that macro.
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that the handle points to a valid item that
    /// has not been removed from the pool.
    ///
    /// The caller must guarantee that the target object is in a state where it is
    /// valid to create a shared reference to it (i.e. no concurrent `&mut` exclusive
    /// references exist.)
    ///
    /// The caller must guarantee that the callback input and output references
    /// point to the same object.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::fmt::Display;
    ///
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    /// let value_handle = pool.insert("Test".to_string());
    ///
    /// // Cast to trait object.
    /// let display_handle: blind_pool::RawPooled<dyn Display> =
    ///     unsafe { value_handle.__private_cast_dyn_with_fn(|x| x as &dyn Display) };
    ///
    /// // Can access as Display trait object.
    /// // SAFETY: The pointer is valid and contains a String which implements Display.
    /// let display_ref: &dyn Display = unsafe { display_handle.ptr().as_ref() };
    /// assert_eq!(format!("{}", display_ref), "Test");
    /// ```
    #[must_use]
    #[inline]
    #[doc(hidden)]
    pub unsafe fn __private_cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> RawPooled<U>
    where
        F: FnOnce(&T) -> &U,
    {
        // SAFETY: Forwarding safety requirements to the caller.
        let inner_pooled_new = unsafe { self.inner.__private_cast_dyn_with_fn(cast_fn) };

        RawPooled {
            layout: self.layout,
            inner: inner_pooled_new,
        }
    }

    /// Converts this raw pooled handle to a trait object using a mutable reference cast.
    ///
    /// This method allows casting from `RawPooled<T>` to `RawPooled<dyn Trait>` where `T`
    /// implements the trait, while maintaining exclusive access semantics. The resulting
    /// handle maintains all the same semantics, but dereferences to the trait object
    /// instead of the concrete type.
    ///
    /// This is intended for use by `PooledMut<T>` and `LocalPooledMut<T>` to maintain
    /// proper borrowing semantics during trait object casting.
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that the raw pooled object
    /// handle points to an item that is still present in the pool.
    ///
    /// The caller must guarantee that the target object is in a state where it is
    /// valid to create an exclusive reference to it (i.e. no concurrent `&` shared
    /// or `&mut` exclusive references exist).
    ///
    /// The caller must guarantee that the callback input and output references
    /// point to the same object.
    #[must_use]
    #[inline]
    #[doc(hidden)]
    pub unsafe fn __private_cast_dyn_with_fn_mut<U: ?Sized, F>(self, cast_fn: F) -> RawPooled<U>
    where
        F: FnOnce(&mut T) -> &mut U,
    {
        // SAFETY: Forwarding safety requirements to the caller.
        let inner_pooled_new = unsafe { self.inner.__private_cast_dyn_with_fn_mut(cast_fn) };

        RawPooled {
            layout: self.layout,
            inner: inner_pooled_new,
        }
    }

    /// Returns a reference to the underlying opaque pool handle.
    ///
    /// This method is intended for internal use by the higher-level pooled types
    /// to leverage the new convenience methods (Deref, `as_pin`) in `opaque_pool`.
    #[must_use]
    #[inline]
    pub(crate) fn opaque_handle(&self) -> &OpaquePooled<T> {
        &self.inner
    }
}

impl<T: ?Sized> Copy for RawPooled<T> {}

impl<T: ?Sized> Clone for RawPooled<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl fmt::Debug for RawBlindPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawBlindPool")
            .field("pools", &self.pools)
            .field("drop_policy", &self.drop_policy)
            .finish()
    }
}

impl<T: ?Sized> fmt::Debug for RawPooled<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawPooled")
            .field("type_name", &std::any::type_name::<T>())
            .field("layout", &self.layout)
            .field("ptr", &self.inner.ptr())
            .finish()
    }
}

// SAFETY: RawBlindPool can exist on any thread, as it does not reference any thread-specific data.
// If a !Send item is inserted, the returned handle can only be used on the same thread, so that
// item is bound to a single thread even if the pool itself is not.
unsafe impl Send for RawBlindPool {}

// SAFETY: RawPooled<T> is just a fancy reference, so its thread-safety is entirely driven by the
// underlying type T and the presence of the `Sync` auto trait on it.
unsafe impl<T: ?Sized + Sync> Send for RawPooled<T> {}

// SAFETY: RawPooled<T> is just a fancy reference, so its thread-safety is entirely driven by the
// underlying type T and the presence of the `Sync` auto trait on it.
unsafe impl<T: ?Sized + Sync> Sync for RawPooled<T> {}

/// A handle to a value stored in a [`RawBlindPool`] with exclusive ownership guarantees.
///
/// Unlike [`RawPooled<T>`], this handle cannot be copied or cloned, ensuring that only one
/// handle can exist for each pool item. This enables safe removal methods that consume the
/// handle, preventing double-use bugs that could lead to undefined behavior.
///
/// The handle provides access to the stored value via a pointer, similar to [`RawPooled<T>`],
/// but with the additional safety guarantee of exclusive ownership.
///
/// # Thread safety
///
/// When `T` is [`Sync`], the handle is thread-safe and can be freely moved and shared between
/// threads, providing safe concurrent access to the underlying data.
///
/// When `T` is not [`Sync`], the handle is single-threaded and cannot be moved between threads
/// or shared between threads, preventing unsafe access to non-thread-safe data.
pub struct RawPooledMut<T: ?Sized> {
    /// The memory layout of the stored item. This is used to identify which internal
    /// pool the item belongs to.
    layout: Layout,

    /// The exclusive handle from the internal opaque pool.
    inner: PooledMut<T>,
}

impl<T: ?Sized> RawPooledMut<T> {
    /// Returns a pointer to the inserted value.
    ///
    /// This is the only way to access the value stored in the pool. The owner of the handle has
    /// exclusive access to the value and may both read and write and may create both `&` shared
    /// and `&mut` exclusive references to the item.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    ///
    /// let pooled_mut = pool.insert_mut(2.5159_f64);
    ///
    /// // Read data back from the memory.
    /// // SAFETY: The pointer is valid and the memory contains the value we just inserted.
    /// let value = unsafe { *pooled_mut.ptr().as_ref() };
    /// assert_eq!(value, 2.5159);
    /// ```
    #[must_use]
    #[inline]
    pub fn ptr(&self) -> NonNull<T> {
        self.inner.ptr()
    }

    /// Erases the type information from this `RawPooledMut<T>` handle,
    /// returning a `RawPooledMut<()>`.
    ///
    /// This is useful when you want to store handles of different types in the same collection
    /// or pass them to code that doesn't need to know the specific type.
    ///
    /// The handle remains functionally equivalent and can still be used to remove the item
    /// from the pool safely. The only change is the removal of the type information.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    /// let pooled_mut = pool.insert_mut("Test".to_string());
    ///
    /// // Erase the type information.
    /// let erased = pooled_mut.erase();
    ///
    /// // Can still be removed safely.
    /// pool.remove_mut(erased);
    /// ```
    #[must_use]
    #[inline]
    pub fn erase(self) -> RawPooledMut<()> {
        RawPooledMut {
            layout: self.layout,
            inner: self.inner.erase(),
        }
    }

    /// Converts this exclusive handle to a shared handle.
    ///
    /// This consumes the `RawPooledMut<T>` and returns a `RawPooled<T>` that can be copied
    /// and cloned. Use this when you no longer need the exclusive ownership guarantees
    /// and want to share the handle.
    ///
    /// Note that after calling this method, you lose the safety guarantees of exclusive
    /// ownership and must use unsafe removal methods.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    /// let pooled_mut = pool.insert_mut("Test".to_string());
    ///
    /// // Convert to shared handle.
    /// let shared = pooled_mut.into_shared();
    ///
    /// // Can now copy the handle.
    /// let shared_copy = shared;
    ///
    /// // But removal requires unsafe code again.
    /// unsafe { pool.remove(&shared_copy) };
    /// ```
    #[must_use]
    #[inline]
    pub fn into_shared(self) -> RawPooled<T> {
        RawPooled {
            layout: self.layout,
            inner: self.inner.into_shared(),
        }
    }
}

impl<T: ?Sized> fmt::Debug for RawPooledMut<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawPooledMut")
            .field("layout", &self.layout)
            .field("inner", &self.inner)
            .finish()
    }
}

// SAFETY: RawPooledMut<T> is just a fancy reference with exclusive ownership, so its thread-safety
// is entirely driven by the underlying type T and the presence of the `Sync` auto trait on it.
unsafe impl<T: ?Sized + Sync> Send for RawPooledMut<T> {}

// SAFETY: RawPooledMut<T> is just a fancy reference with exclusive ownership, so its thread-safety
// is entirely driven by the underlying type T and the presence of the `Sync` auto trait on it.
unsafe impl<T: ?Sized + Sync> Sync for RawPooledMut<T> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_insert_remove() {
        let mut pool = RawBlindPool::new();
        let _pooled = pool.insert(42_u32);
        // SAFETY: This pooled handle was just created and has never been used for removal before.
        unsafe {
            pool.remove(&_pooled);
        }
    }

    #[test]
    fn two_items_same_type() {
        let mut pool = RawBlindPool::new();
        let _pooled1 = pool.insert(42_u32);
        let _pooled2 = pool.insert(43_u32);
        // SAFETY: This pooled handle was just created and has never been used for removal before.
        unsafe {
            pool.remove(&_pooled1);
        }
        // SAFETY: This pooled handle was just created and has never been used for removal before.
        unsafe {
            pool.remove(&_pooled2);
        }
    }

    #[test]
    fn two_items_different_types() {
        let mut pool = RawBlindPool::new();
        let _pooled1 = pool.insert(42_u32);
        let _pooled2 = pool.insert(43_u64);
        // SAFETY: This pooled handle was just created and has never been used for removal before.
        unsafe {
            pool.remove(&_pooled1);
        }
        // SAFETY: This pooled handle was just created and has never been used for removal before.
        unsafe {
            pool.remove(&_pooled2);
        }
    }

    #[test]
    fn smoke_test() {
        let mut pool = RawBlindPool::new();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());

        let pooled_u32 = pool.insert(42_u32);
        let pooled_u64 = pool.insert(43_u64);
        let pooled_f32 = pool.insert(2.5_f32);

        assert_eq!(pool.len(), 3);
        assert!(!pool.is_empty());

        // SAFETY: The pointers are valid and contain the values we just inserted.
        let u32_val = unsafe { pooled_u32.ptr().read() };
        // SAFETY: The pointers are valid and contain the values we just inserted.
        let u64_val = unsafe { pooled_u64.ptr().read() };
        // SAFETY: The pointers are valid and contain the values we just inserted.
        let f32_val = unsafe { pooled_f32.ptr().read() };
        assert_eq!(u32_val, 42);
        assert_eq!(u64_val, 43);
        assert!((f32_val - 2.5).abs() < f32::EPSILON);

        // SAFETY: This pooled handle was just created and has never been used for removal before.
        unsafe {
            pool.remove(&pooled_u32);
        }
        // SAFETY: This pooled handle was just created and has never been used for removal before.
        unsafe {
            pool.remove(&pooled_u64);
        }
        // SAFETY: This pooled handle was just created and has never been used for removal before.
        unsafe {
            pool.remove(&pooled_f32);
        }

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn different_types_same_layout() {
        let mut pool = RawBlindPool::new();

        // These types have the same layout (both are 4 bytes, 4-byte aligned).
        let pooled_u32 = pool.insert(42_u32);
        let pooled_i32 = pool.insert(-42_i32);
        let pooled_f32 = pool.insert(2.5_f32);

        assert_eq!(pool.len(), 3);

        // SAFETY: The pointers are valid and contain the values we just inserted.
        let u32_val = unsafe { pooled_u32.ptr().read() };
        // SAFETY: The pointers are valid and contain the values we just inserted.
        let i32_val = unsafe { pooled_i32.ptr().read() };
        // SAFETY: The pointers are valid and contain the values we just inserted.
        let f32_val = unsafe { pooled_f32.ptr().read() };
        assert_eq!(u32_val, 42);
        assert_eq!(i32_val, -42);
        assert!((f32_val - 2.5).abs() < f32::EPSILON);

        // SAFETY: This pooled handle was just created and has never been used for removal before.
        unsafe {
            pool.remove(&pooled_u32);
        }
        // SAFETY: This pooled handle was just created and has never been used for removal before.
        unsafe {
            pool.remove(&pooled_i32);
        }
        // SAFETY: This pooled handle was just created and has never been used for removal before.
        unsafe {
            pool.remove(&pooled_f32);
        }

        assert!(pool.is_empty());
    }

    #[test]
    fn builder_with_drop_policy() {
        let pool = RawBlindPool::builder()
            .drop_policy(DropPolicy::MustNotDropItems)
            .build();

        // Pool should not panic when dropped if empty.
        drop(pool);
    }

    #[test]
    #[should_panic(expected = "BlindPool dropped while still containing items")]
    fn drop_policy_must_not_drop_panics_when_not_empty() {
        let mut pool = RawBlindPool::builder()
            .drop_policy(DropPolicy::MustNotDropItems)
            .build();

        let _pooled = pool.insert(42_u32);

        // Pool should panic when dropped with items.
        drop(pool);
    }

    #[test]
    fn shrink_to_fit_removes_empty_pools() {
        let mut pool = RawBlindPool::new();

        // Insert items of different types to create multiple internal pools.
        // Use types with different layouts: u8 (1 byte), u64 (8 bytes), [u8; 3] (3 bytes).
        let pooled_u8 = pool.insert(1_u8);
        let pooled_u64 = pool.insert(2_u64);
        _ = pool.insert([1_u8, 2_u8, 3_u8]);

        // Verify we have multiple internal pools.
        assert_eq!(pool.pools.len(), 3);
        assert_eq!(pool.len(), 3);

        // Remove some but not all items (we leave the array).
        // SAFETY: This pooled handle was just created and has never been used for removal before.
        unsafe {
            pool.remove(&pooled_u8);
        }
        // SAFETY: This pooled handle was just created and has never been used for removal before.
        unsafe {
            pool.remove(&pooled_u64);
        }

        assert_eq!(pool.pools.len(), 3); // Internal pools still exist before shrinking.

        // This should clean up empty internal pools.
        pool.shrink_to_fit();

        // Verify that empty internal pools have been removed.
        assert_eq!(pool.pools.len(), 1);
    }

    #[test]
    fn reserve_increases_capacity() {
        let mut pool = RawBlindPool::new();

        // Initially no capacity
        assert_eq!(pool.capacity_of::<u32>(), 0);

        // Reserve capacity before inserting any items
        pool.reserve_for::<u32>(10);
        assert!(pool.capacity_of::<u32>() >= 10);

        // Insert an item - should use the reserved capacity
        let _pooled = pool.insert(42_u32);
        assert!(pool.capacity_of::<u32>() >= 10); // Should still have the reserved capacity

        // SAFETY: This pooled handle was just created and has never been used for removal before.`n        unsafe { pool.remove(&_pooled); }
    }

    #[test]
    fn reserve_with_existing_items() {
        let mut pool = RawBlindPool::new();

        // Insert some items first
        let pooled1 = pool.insert(1_u32);
        let pooled2 = pool.insert(2.5_f64);

        // Current state: 1 u32 item, some f64 capacity
        let current_u32_count = 1; // We know we have 1 u32

        // Reserve additional space for u32 specifically - should ensure capacity for
        // current + additional
        pool.reserve_for::<u32>(10);
        assert!(pool.capacity_of::<u32>() >= current_u32_count + 10);

        // f64 capacity should be unaffected by u32 reserve
        let f64_capacity_before = pool.capacity_of::<f64>();
        // Reserve again - if already sufficient, capacity shouldn't increase
        pool.reserve_for::<u32>(5);
        let f64_capacity_after = pool.capacity_of::<f64>();
        assert_eq!(f64_capacity_before, f64_capacity_after);

        // Verify existing items are still accessible
        // SAFETY: The pointers are valid and contain the values we just inserted.
        unsafe {
            assert_eq!(pooled1.ptr().read(), 1);
        }
        // SAFETY: The pointers are valid and contain the values we just inserted.
        unsafe {
            assert!((pooled2.ptr().read() - 2.5).abs() < f64::EPSILON);
        }

        // SAFETY: This pooled handle was just created and has never been used for removal before.`n        unsafe { pool.remove(&_pooled1); }
        // SAFETY: This pooled handle was just created and has never been used for removal before.`n        unsafe { pool.remove(&_pooled2); }
    }

    #[test]
    fn reserve_zero_does_nothing() {
        let mut pool = RawBlindPool::new();

        // Reserve zero for a type that doesn't exist yet
        pool.reserve_for::<u32>(0);
        assert_eq!(pool.capacity_of::<u32>(), 0);

        // Insert an item and reserve zero
        let _pooled = pool.insert(42_u32);
        let initial_capacity = pool.capacity_of::<u32>();
        pool.reserve_for::<u32>(0);
        assert_eq!(pool.capacity_of::<u32>(), initial_capacity);

        // SAFETY: This pooled handle was just created and has never been used for removal before.`n        unsafe { pool.remove(&_pooled); }
    }

    #[test]
    fn reserve_with_sufficient_capacity_does_nothing() {
        let mut pool = RawBlindPool::new();

        // Reserve initial capacity for u32
        pool.reserve_for::<u32>(10);
        let capacity_after_reserve = pool.capacity_of::<u32>();
        assert!(capacity_after_reserve >= 10);

        // Try to reserve less than what we already have available
        pool.reserve_for::<u32>(5);
        assert_eq!(pool.capacity_of::<u32>(), capacity_after_reserve);

        // Insert an item and reserve less than current capacity
        let _pooled = pool.insert(42_u32);
        pool.reserve_for::<u32>(3);
        assert_eq!(pool.capacity_of::<u32>(), capacity_after_reserve);

        // SAFETY: This pooled handle was just created and has never been used for removal before.`n        unsafe { pool.remove(&_pooled); }
    }

    #[test]
    fn reserve_for_empty_pool_creates_internal_pool() {
        let mut pool = RawBlindPool::new();

        // Reserve for a type on empty pool - should create internal pool
        pool.reserve_for::<u32>(10);
        assert!(pool.capacity_of::<u32>() >= 10);
        assert_eq!(pool.capacity_of::<f64>(), 0); // Other types unaffected

        // Insert an item - should use the reserved capacity
        let initial_capacity = pool.capacity_of::<u32>();
        let _pooled = pool.insert(42_u32);
        assert_eq!(pool.capacity_of::<u32>(), initial_capacity);

        // SAFETY: This pooled handle was just created and has never been used for removal before.`n        unsafe { pool.remove(&_pooled); }
    }

    #[test]
    fn reserve_for_different_types_independent() {
        let mut pool = RawBlindPool::new();

        // Reserve for different types
        pool.reserve_for::<u32>(5);
        pool.reserve_for::<f64>(15);
        pool.reserve_for::<u8>(25);

        // Each type should have its own capacity
        assert!(pool.capacity_of::<u32>() >= 5);
        assert!(pool.capacity_of::<f64>() >= 15);
        assert!(pool.capacity_of::<u8>() >= 25);

        // Insert items and verify capacities remain independent
        let _pooled_u32 = pool.insert(42_u32);
        let _pooled_f64 = pool.insert(2.71_f64); // e approximation instead of pi
        let _pooled_u8 = pool.insert(255_u8);

        // Reserve more for one type - others should be unaffected
        let f64_capacity_before = pool.capacity_of::<f64>();
        let u8_capacity_before = pool.capacity_of::<u8>();

        pool.reserve_for::<u32>(10);

        assert_eq!(pool.capacity_of::<f64>(), f64_capacity_before);
        assert_eq!(pool.capacity_of::<u8>(), u8_capacity_before);

        // SAFETY: This pooled handle was just created and has never been used for removal before.`n        unsafe { pool.remove(&_pooled_u32); }
        // SAFETY: This pooled handle was just created and has never been used for removal before.`n        unsafe { pool.remove(&_pooled_f64); }
        // SAFETY: This pooled handle was just created and has never been used for removal before.`n        unsafe { pool.remove(&_pooled_u8); }
    }

    #[test]
    fn capacity_of_tracks_specific_types() {
        let mut pool = RawBlindPool::new();

        // Initially no capacity for any type
        assert_eq!(pool.capacity_of::<u32>(), 0);
        assert_eq!(pool.capacity_of::<f64>(), 0);
        assert_eq!(pool.capacity_of::<String>(), 0);

        // Insert u32 - should allocate capacity for u32 only
        let _pooled_u32 = pool.insert(42_u32);
        assert!(pool.capacity_of::<u32>() > 0);
        assert_eq!(pool.capacity_of::<f64>(), 0);
        assert_eq!(pool.capacity_of::<String>(), 0);

        // Insert f64 - should allocate capacity for f64 only
        let _pooled_f64 = pool.insert(2.71_f64); // e approximation
        assert!(pool.capacity_of::<u32>() > 0);
        assert!(pool.capacity_of::<f64>() > 0);
        assert_eq!(pool.capacity_of::<String>(), 0);

        // Types with same layout should share capacity (u32 and i32 have same layout)
        let u32_capacity = pool.capacity_of::<u32>();
        let i32_capacity = pool.capacity_of::<i32>();
        assert_eq!(u32_capacity, i32_capacity);

        // SAFETY: This pooled handle was just created and has never been used for removal before.`n        unsafe { pool.remove(&_pooled_u32); }
        // SAFETY: This pooled handle was just created and has never been used for removal before.`n        unsafe { pool.remove(&_pooled_f64); }
    }

    #[test]
    #[should_panic]
    fn reserve_overflow_panics() {
        let mut pool = RawBlindPool::new();

        // Insert one item to make len() = 1
        let _key = pool.insert(42_u32);

        // Try to reserve usize::MAX more items for u32.
        // This will cause overflow during capacity calculation in the internal pool
        pool.reserve_for::<u32>(usize::MAX);
    }

    #[test]
    fn erase_type_information() {
        let mut pool = RawBlindPool::new();

        let pooled = pool.insert(42_u64);
        let erased = pooled.erase();

        // SAFETY: We know this contains a u64.
        let value = unsafe { erased.ptr().cast::<u64>().read() };
        assert_eq!(value, 42);

        // SAFETY: This pooled handle was just created and has never been used for removal before.
        unsafe {
            pool.remove(&erased);
        }
        assert!(pool.is_empty());
    }

    #[test]
    fn works_with_drop_types() {
        let mut pool = RawBlindPool::new();

        // Test with String - a type that implements Drop
        let test_string = "Hello, World!".to_string();
        let pooled_string = pool.insert(test_string);

        // SAFETY: This pooled handle was just created and has never been used for removal before.
        unsafe {
            pool.remove(&pooled_string);
        }

        // Test with Vec - another type that implements Drop
        let test_vec = vec![1, 2, 3, 4, 5];
        let pooled_vec = pool.insert(test_vec);

        // SAFETY: This pooled handle was just created and has never been used for removal before.
        unsafe {
            pool.remove(&pooled_vec);
        }

        assert!(pool.is_empty());
    }

    #[test]
    fn trait_object_usage() {
        // Define a trait for testing.
        trait Printable {
            fn print_info(&self) -> String;
        }

        #[derive(Debug)]
        struct Book {
            title: String,
            pages: u32,
        }

        impl Printable for Book {
            fn print_info(&self) -> String {
                format!("Book: '{}' ({} pages)", self.title, self.pages)
            }
        }

        let mut pool = RawBlindPool::new();

        // Insert a book into the pool.
        let book = Book {
            title: "The Rust Programming Language".to_string(),
            pages: 552,
        };

        let pooled_book = pool.insert(book);

        // Use item as trait object.
        // SAFETY: The pointer is valid and points to a Book that we just inserted.
        unsafe {
            let book_ref: &Book = pooled_book.ptr().as_ref();
            let printable: &dyn Printable = book_ref;
            assert_eq!(
                printable.print_info(),
                "Book: 'The Rust Programming Language' (552 pages)"
            );
        }

        // SAFETY: This pooled handle was just created and has never been used for removal before.`n        unsafe { pool.remove(&_pooled_book); }
    }

    #[test]
    fn trait_object_with_mutable_references() {
        trait Modifiable {
            fn modify(&mut self, factor: f64);
            fn get_value(&self) -> f64;
        }

        #[derive(Debug)]
        struct Temperature {
            celsius: f64,
        }

        impl Modifiable for Temperature {
            fn modify(&mut self, factor: f64) {
                self.celsius *= factor;
            }

            fn get_value(&self) -> f64 {
                self.celsius
            }
        }

        let mut pool = RawBlindPool::new();

        let temp = Temperature { celsius: 25.0 };
        let pooled_temp = pool.insert(temp);

        // Test mutable trait objects.
        // SAFETY: The pointer is valid and points to a Temperature that we just inserted.
        unsafe {
            let temp_ref: &mut Temperature = pooled_temp.ptr().as_mut();
            let modifiable: &mut dyn Modifiable = temp_ref;

            assert!((modifiable.get_value() - 25.0).abs() < f64::EPSILON);
            modifiable.modify(2.0);
            assert!((modifiable.get_value() - 50.0).abs() < f64::EPSILON);
        }

        // Verify changes persisted.
        // SAFETY: The pointer is valid and points to the object we modified.
        unsafe {
            let temp_ref: &Temperature = pooled_temp.ptr().as_ref();
            assert!((temp_ref.celsius - 50.0).abs() < f64::EPSILON);
        }

        // SAFETY: This pooled handle was just created and has never been used for removal before.`n        unsafe { pool.remove(&_pooled_temp); }
    }

    #[cfg(test)]
    mod static_assertions {
        use static_assertions::{assert_impl_all, assert_not_impl_any};

        use super::{RawBlindPool, RawPooled};
        use crate::RawBlindPoolBuilder;

        #[test]
        fn thread_mobility_assertions() {
            // RawBlindPool should be thread-mobile (Send) but not thread-safe (Sync)
            assert_impl_all!(RawBlindPool: Send);
            assert_not_impl_any!(RawBlindPool: Sync);

            // RawBlindPoolBuilder should be thread-mobile (Send) but not thread-safe (Sync)
            assert_impl_all!(RawBlindPoolBuilder: Send);
            assert_not_impl_any!(RawBlindPoolBuilder: Sync);

            // RawPooled<T> should be Send+Sync if T is Sync, single-threaded otherwise
            assert_impl_all!(RawPooled<u32>: Send, Sync); // u32 is Sync
            assert_impl_all!(RawPooled<String>: Send, Sync); // String is Sync
            assert_impl_all!(RawPooled<Vec<u8>>: Send, Sync); // Vec<u8> is Sync

            // RawPooled<T> should be single-threaded when T is not Sync
            use std::rc::Rc;
            assert_not_impl_any!(RawPooled<Rc<u32>>: Send, Sync); // Rc is not Sync

            use std::cell::RefCell;
            assert_not_impl_any!(RawPooled<RefCell<u32>>: Send, Sync); // RefCell is not Sync

            // Type-erased handles should be Send and Sync (() is Sync)
            assert_impl_all!(RawPooled<()>: Send, Sync);

            // RawBlindPool and RawPooled<T> should always be Unpin regardless of T
            assert_impl_all!(RawBlindPool: Unpin);
            assert_impl_all!(RawBlindPoolBuilder: Unpin);

            assert_impl_all!(RawPooled<u32>: Unpin);
            assert_impl_all!(RawPooled<String>: Unpin);
            assert_impl_all!(RawPooled<Vec<u8>>: Unpin);
            assert_impl_all!(RawPooled<Rc<u32>>: Unpin);
            assert_impl_all!(RawPooled<RefCell<u32>>: Unpin);
            assert_impl_all!(RawPooled<()>: Unpin);

            // Even with non-Unpin types, RawPooled should still be Unpin
            use std::marker::PhantomPinned;
            assert_impl_all!(RawPooled<PhantomPinned>: Unpin);
        }
    }

    #[test]
    fn insert_with_basic_functionality() {
        let mut pool = RawBlindPool::new();

        // SAFETY: We properly initialize the u32 value.
        let pooled = unsafe {
            pool.insert_with(|uninit: &mut MaybeUninit<u32>| {
                uninit.write(42);
            })
        };

        // Verify the value was properly initialized.
        // SAFETY: The pointer is valid and points to the u32 value we just initialized.
        unsafe {
            let value = pooled.ptr().read();
            assert_eq!(value, 42);
        }

        assert_eq!(pool.len(), 1);
        // SAFETY: This pooled handle was just created and has never been used for removal before.
        unsafe {
            pool.remove(&pooled);
        }
        assert_eq!(pool.len(), 0);
    }
}
