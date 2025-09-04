use std::alloc::Layout;
use std::mem::MaybeUninit;
use std::sync::{Arc, Mutex, MutexGuard};

use foldhash::HashMap;

use crate::{ERR_POISONED_LOCK, PooledMut, RawOpaquePool, RawOpaquePoolSend};

/// A reference-counting object pool that accepts any type of object.
///
/// All values in the pool remain pinned for their entire lifetime.
///
/// The pool automatically expands its capacity when needed.
///
/// # Lifetime management
///
/// The pool type itself acts as a handle - any clones of it are functionally equivalent,
/// similar to `Arc`.
///
/// When inserting an object into the pool, a handle to the object is returned.
/// The object is removed from the pool when the last remaining handle to the object
/// is dropped (`Arc`-like behavior).
///
/// # Thread safety
///
/// The pool is thread-safe (`Send` and `Sync`) and requires that any inserted items are `Send`.
#[derive(Debug, Default)]
pub struct BlindPool {
    // Internal pools, one for each unique memory layout encountered.
    //
    // The pool type itself is just a handle around the inner pool,
    // which is reference-counted and mutex-guarded. The inner pool
    // will only ever be dropped once all items have been removed from
    // it and no more `OpaquePool` instances exist that point to it.
    //
    // This also implies that `DropPolicy` has no meaning for this
    // pool configuration, as the pool can never be dropped if it has
    // contents (as dropping the handles of pooled objects will remove
    // them from the pool, while keeping the pool alive until then).
    pools: Arc<Mutex<PoolMap>>,
}

impl BlindPool {
    /// Creates a new instance of the pool.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The number of objects currently in the pool.
    #[must_use]
    pub fn len(&self) -> usize {
        let pools = self.pools.lock().expect(ERR_POISONED_LOCK);

        pools
            .values()
            .map(|pool| pool.lock().expect(ERR_POISONED_LOCK).len())
            .sum()
    }

    /// The total capacity of the pool for objects of type `T`.
    ///
    /// This is the maximum number of objects of this type that the pool can contain without
    /// capacity extension. The pool will automatically extend its capacity if more than
    /// this many objects of type `T` are inserted. Capacity may be shared between different
    /// types of objects.
    #[must_use]
    pub fn capacity_for<T: Send>(&self) -> usize {
        let layout = Layout::new::<T>();

        let pools = self.pools.lock().expect(ERR_POISONED_LOCK);

        pools
            .get(&layout)
            .map(|pool| pool.lock().expect(ERR_POISONED_LOCK).capacity())
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
    pub fn reserve_for<T: Send>(&mut self, additional: usize) {
        let mut pools = self.pools.lock().expect(ERR_POISONED_LOCK);

        let pool = ensure_inner_pool::<T>(&mut pools);

        pool.lock().expect(ERR_POISONED_LOCK).reserve(additional);
    }

    /// Drops unused pool capacity to reduce memory usage.
    ///
    /// There is no guarantee that any unused capacity can be dropped. The exact outcome depends
    /// on the specific pool structure and which objects remain in the pool.
    pub fn shrink_to_fit(&mut self) {
        let mut pools = self.pools.lock().expect(ERR_POISONED_LOCK);

        for pool in pools.values_mut() {
            pool.lock().expect(ERR_POISONED_LOCK).shrink_to_fit();
        }
    }

    /// Inserts an object into the pool and returns a handle to it.
    pub fn insert<T: Send>(&mut self, value: T) -> PooledMut<T> {
        let mut pools = self.pools.lock().expect(ERR_POISONED_LOCK);

        let pool = ensure_inner_pool::<T>(&mut pools);

        // SAFETY: inner pool selector guarantees matching layout.
        let inner_handle = unsafe {
            pool.lock()
                .expect(ERR_POISONED_LOCK)
                .insert_unchecked(value)
        };

        PooledMut::new(inner_handle, Arc::clone(pool))
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
    pub unsafe fn insert_with<T: Send, F>(&mut self, f: F) -> PooledMut<T>
    where
        F: FnOnce(&mut MaybeUninit<T>),
    {
        let mut pools = self.pools.lock().expect(ERR_POISONED_LOCK);

        let pool = ensure_inner_pool::<T>(&mut pools);

        // SAFETY: inner pool selector guarantees matching layout.
        // Initialization guarantee is forwarded from the caller.
        let inner_handle = unsafe {
            pool.lock()
                .expect(ERR_POISONED_LOCK)
                .insert_with_unchecked(f)
        };

        PooledMut::new(inner_handle, Arc::clone(pool))
    }
}

// Each inner pool is separately locked because those locks are used by the handles to
// remove the specific object from the specific pool in a thread-safe manner. The pool
// itself only ever takes those locks while holding the map lock, ensuring correct lock ordering.
type PoolMap = HashMap<Layout, Arc<Mutex<RawOpaquePoolSend>>>;

fn ensure_inner_pool<'a, T: Send>(
    pools: &'a mut MutexGuard<'_, PoolMap>,
) -> &'a Arc<Mutex<RawOpaquePoolSend>> {
    let layout = Layout::new::<T>();

    pools.entry(layout).or_insert_with_key(|layout| {
        // SAFETY: We always require `T: Send`.
        let inner = unsafe { RawOpaquePoolSend::new(RawOpaquePool::with_layout(*layout)) };

        Arc::new(Mutex::new(inner))
    })
}
