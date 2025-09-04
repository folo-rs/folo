use std::alloc::Layout;
use std::mem::MaybeUninit;

use foldhash::{HashMap, HashMapExt};

use crate::{DropPolicy, RawBlindPooled, RawBlindPooledMut, RawOpaquePool};

/// An object pool that accepts any type of object.
///
/// All values in the pool remain pinned for their entire lifetime.
///
/// The pool automatically expands its capacity when needed.
///
/// # Thread safety
///
/// The pool is single-threaded, though if all the objects inserted are `Send` then the owner of
/// the pool is allowed to treat the pool itself as `Send` (but must do so via a wrapper type that
/// implements `Send` using unsafe code).
#[derive(Debug)]
pub struct RawBlindPool {
    /// Internal pools, one for each unique memory layout encountered.
    pools: HashMap<Layout, RawOpaquePool>,

    drop_policy: DropPolicy,
}

impl RawBlindPool {
    /// Creates a new instance of the pool with the default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self {
            pools: HashMap::new(),
            drop_policy: DropPolicy::default(),
        }
    }

    /// The number of objects currently in the pool.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pools.values().map(RawOpaquePool::len).sum()
    }

    /// The total capacity of the pool for objects of type `T`.
    ///
    /// This is the maximum number of objects of this type that the pool can contain without
    /// capacity extension. The pool will automatically extend its capacity if more than
    /// this many objects of type `T` are inserted. Capacity may be shared between different
    /// types of objects.
    #[must_use]
    pub fn capacity_for<T>(&self) -> usize {
        self.inner_pool_of::<T>()
            .map(RawOpaquePool::capacity)
            .unwrap_or_default()
    }

    /// Whether the pool contains zero objects.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Ensures that the pool has capacity for at least `additional` more objects of type `T`.
    ///
    /// # Panics
    ///
    /// Panics if the new capacity would exceed the size of virtual memory.
    pub fn reserve_for<T>(&mut self, additional: usize) {
        self.inner_pool_of_mut::<T>().reserve(additional);
    }

    /// Drops unused pool capacity to reduce memory usage.
    ///
    /// There is no guarantee that any unused capacity can be dropped. The exact outcome depends
    /// on the specific pool structure and which objects remain in the pool.
    pub fn shrink_to_fit(&mut self) {
        for pool in self.pools.values_mut() {
            pool.shrink_to_fit();
        }
    }

    /// Inserts an object into the pool and returns a handle to it.
    pub fn insert<T>(&mut self, value: T) -> RawBlindPooledMut<T> {
        let layout = Layout::new::<T>();
        let pool = self.inner_pool_mut(layout);

        // SAFETY: inner pool selector guarantees matching layout.
        let inner_handle = unsafe { pool.insert_unchecked(value) };

        RawBlindPooledMut::new(layout, inner_handle)
    }

    /// Inserts an object into the pool via closure and returns a handle to it.
    ///
    /// This method allows the caller to partially initialize the object, skipping any `MaybeUninit`
    /// fields that are intentionally not initialized at insertion time. This can make insertion of
    /// objects containing `MaybeUninit` fields faster, although requires unsafe code to implement.
    ///
    /// This method is NOT faster than `insert()` for fully initialized objects.
    /// Prefer `insert()` for a better safety posture if you do not intend to
    /// skip initialization of any `MaybeUninit` fields.
    ///
    /// # Safety
    ///
    /// The closure must correctly initialize the object. All fields that
    /// are not `MaybeUninit` must be initialized when the closure returns.
    pub unsafe fn insert_with<T, F>(&mut self, f: F) -> RawBlindPooledMut<T>
    where
        F: FnOnce(&mut MaybeUninit<T>),
    {
        let layout = Layout::new::<T>();
        let pool = self.inner_pool_mut(layout);

        // SAFETY: inner pool selector guarantees matching layout.
        // Initialization guarantee is forwarded from the caller.
        let inner_handle = unsafe { pool.insert_with_unchecked(f) };

        RawBlindPooledMut::new(layout, inner_handle)
    }

    /// Removes an object from the pool, dropping it.
    ///
    /// # Panics
    ///
    /// Panics if the handle does not reference an object in this pool.
    pub fn remove_mut<T: ?Sized>(&mut self, handle: RawBlindPooledMut<T>) {
        let pool = self.inner_pool_mut(handle.layout());

        pool.remove_mut(handle.into_inner());
    }

    /// Removes an object from the pool, dropping it.
    ///
    /// # Panics
    ///
    /// Panics if the handle does not reference an object in this pool.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the handle belongs to this pool and that the object it
    /// references has not already been removed from the pool.
    pub unsafe fn remove<T: ?Sized>(&mut self, handle: RawBlindPooled<T>) {
        let pool = self.inner_pool_mut(handle.layout());

        // SAFETY: Forwarding safety guarantees from the caller.
        unsafe {
            pool.remove(handle.into_inner());
        }
    }

    /// Removes an object from the pool and returns it.
    ///
    /// # Panics
    ///
    /// Panics if the handle does not reference an object in this pool.
    ///
    /// Panics if the object handle has been type-erased (`RawBlindPooledMut<()>`).
    #[must_use]
    pub fn remove_mut_unpin<T: Unpin>(&mut self, handle: RawBlindPooledMut<T>) -> T {
        // We would rather prefer to check for `RawBlindPooledMut<()>` specifically but
        // that would imply specialization or `T: 'static` or TypeId shenanigans.
        // This is good enough because type-erasing a handle is the only way to get a
        // handle to a ZST anyway because the slab does not even support ZSTs.
        assert_ne!(
            size_of::<T>(),
            0,
            "cannot remove_mut_unpin() from a blind pool through a type-erased handle"
        );

        let pool = self.inner_pool_of_mut::<T>();

        pool.remove_mut_unpin(handle.into_inner())
    }

    /// Removes an object from the pool and returns it.
    ///
    /// # Panics
    ///
    /// Panics if the handle does not reference an existing object in this pool.
    ///
    /// Panics if the object handle has been type-erased (`RawPooled<()>`).
    ///
    /// # Safety
    ///
    /// The caller must ensure that the handle belongs to this pool and that the object it
    /// references has not already been removed from the pool.
    #[must_use]
    pub unsafe fn remove_unpin<T: Unpin>(&mut self, handle: RawBlindPooled<T>) -> T {
        // We would rather prefer to check for `RawPooled<()>` specifically but
        // that would imply specialization or `T: 'static` or TypeId shenanigans.
        // This is good enough because type-erasing a handle is the only way to get a
        // handle to a ZST anyway because the slab does not even support ZSTs.
        assert_ne!(
            size_of::<T>(),
            0,
            "cannot remove_unpin() from a blind pool through a type-erased handle"
        );

        let pool = self.inner_pool_of_mut::<T>();

        // SAFETY: Forwarding safety guarantees from the caller.
        unsafe { pool.remove_unpin(handle.into_inner()) }
    }

    fn inner_pool_of<T>(&self) -> Option<&RawOpaquePool> {
        let layout = Layout::new::<T>();

        self.pools.get(&layout)
    }

    fn inner_pool_of_mut<T>(&mut self) -> &mut RawOpaquePool {
        let layout = Layout::new::<T>();

        self.inner_pool_mut(layout)
    }

    fn inner_pool_mut(&mut self, layout: Layout) -> &mut RawOpaquePool {
        self.pools.entry(layout).or_insert_with_key(|layout| {
            RawOpaquePool::builder()
                .drop_policy(self.drop_policy)
                .layout(*layout)
                .build()
        })
    }
}

impl Default for RawBlindPool {
    fn default() -> Self {
        Self::new()
    }
}
