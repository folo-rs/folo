use std::fmt;
use std::iter::FusedIterator;
use std::mem::MaybeUninit;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use crate::{
    ERR_POISONED_LOCK, PooledMut, RawOpaquePool, RawOpaquePoolIterator, RawOpaquePoolSend,
};

/// A pool of reference-counted objects of type `T`.
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
/// The pool is thread-safe (`Send` and `Sync`) and requires `T: Send`.
pub struct PinnedPool<T: Send + 'static> {
    // We require 'static from any inserted values because the pool
    // does not enforce any Rust lifetime semantics, only reference counts.
    //
    // The pool type itself is just a handle around the inner opaque pool,
    // which is reference-counted and mutex-guarded. The inner pool
    // will only ever be dropped once all items have been removed from
    // it and no more `PinnedPool` instances exist that point to it.
    //
    // This also implies that `DropPolicy` has no meaning for this
    // pool configuration, as the pool can never be dropped if it has
    // contents (as dropping the handles of pooled objects will remove
    // them from the pool, while keeping the pool alive until then).
    inner: Arc<Mutex<RawOpaquePoolSend>>,
    _phantom: std::marker::PhantomData<T>,
}

impl<T> PinnedPool<T>
where
    T: Send + 'static,
{
    /// Creates a new pool for objects of type `T`.
    #[must_use]
    pub fn new() -> Self {
        let inner = RawOpaquePool::with_layout_of::<T>();

        // SAFETY: All insertion methods require `T: Send`.
        let inner = unsafe { RawOpaquePoolSend::new(inner) };

        Self {
            inner: Arc::new(Mutex::new(inner)),
            _phantom: std::marker::PhantomData,
        }
    }

    /// The number of objects currently in the pool.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.lock().expect(ERR_POISONED_LOCK).len()
    }

    /// The total capacity of the pool.
    ///
    /// This is the maximum number of objects that the pool can contain without capacity extension.
    /// The pool will automatically extend its capacity if more than this many objects are inserted.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.inner.lock().expect(ERR_POISONED_LOCK).capacity()
    }

    /// Whether the pool contains zero objects.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.lock().expect(ERR_POISONED_LOCK).is_empty()
    }

    /// Ensures that the pool has capacity for at least `additional` more objects.
    ///
    /// # Panics
    ///
    /// Panics if the new capacity would exceed the size of virtual memory.
    pub fn reserve(&mut self, additional: usize) {
        self.inner
            .lock()
            .expect(ERR_POISONED_LOCK)
            .reserve(additional);
    }

    /// Drops unused pool capacity to reduce memory usage.
    ///
    /// There is no guarantee that any unused capacity can be dropped. The exact outcome depends
    /// on the specific pool structure and which objects remain in the pool.
    pub fn shrink_to_fit(&mut self) {
        self.inner.lock().expect(ERR_POISONED_LOCK).shrink_to_fit();
    }

    /// Inserts an object into the pool and returns a handle to it.
    pub fn insert(&mut self, value: T) -> PooledMut<T> {
        let inner = self.inner.lock().expect(ERR_POISONED_LOCK).insert(value);

        PooledMut::new(inner, Arc::clone(&self.inner))
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
    pub unsafe fn insert_with<F>(&mut self, f: F) -> PooledMut<T>
    where
        F: FnOnce(&mut MaybeUninit<T>),
    {
        // SAFETY: Forwarding safety guarantees from caller.
        let inner = unsafe { self.inner.lock().expect(ERR_POISONED_LOCK).insert_with(f) };

        PooledMut::new(inner, Arc::clone(&self.inner))
    }

    /// Calls a closure with an iterator over all objects in the pool.
    ///
    /// The iterator only yields pointers to the objects, not references, because the pool
    /// does not have the authority to create references to its contents as user code may
    /// concurrently be holding a conflicting exclusive reference via `PooledMut<T>`.
    ///
    /// Therefore, obtaining actual references to pool contents via iteration is only possible
    /// by using the pointer to create such references in unsafe code and relies on the caller
    /// guaranteeing that no conflicting exclusive references exist.
    ///
    /// # Mutual exclusion
    ///
    /// The pool is locked for the entire duration of the closure, ensuring that objects
    /// cannot be removed while iteration is in progress. This guarantees that all pointers
    /// yielded by the iterator remain valid for the duration of the closure.
    ///
    /// # Examples
    ///
    /// ```
    /// # use infinity_pool::PinnedPool;
    /// let mut pool = PinnedPool::<u32>::new();
    /// let _handle1 = pool.insert(42u32);
    /// let _handle2 = pool.insert(100u32);
    ///
    /// // Safe iteration with guaranteed pointer validity
    /// pool.with_iter(|iter| {
    ///     for ptr in iter {
    ///         // SAFETY: We know these are u32 pointers from this pool
    ///         let value = unsafe { ptr.as_ref() };
    ///         println!("Value: {}", value);
    ///     }
    /// });
    ///
    /// // Collect values safely
    /// let values: Vec<u32> =
    ///     pool.with_iter(|iter| iter.map(|ptr| unsafe { *ptr.as_ref() }).collect());
    /// ```
    pub fn with_iter<F, R>(&self, f: F) -> R
    where
        F: FnOnce(PinnedPoolIterator<'_, T>) -> R,
    {
        let guard = self.inner.lock().expect(ERR_POISONED_LOCK);
        let iter = PinnedPoolIterator::new(&guard);
        f(iter)
    }
}

impl<T> Clone for PinnedPool<T>
where
    T: Send,
{
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<T> Default for PinnedPool<T>
where
    T: Send,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<T> fmt::Debug for PinnedPool<T>
where
    T: Send,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PinnedPool")
            .field("inner", &self.inner)
            .finish()
    }
}

/// Iterator over all objects in a pinned pool.
///
/// The iterator only yields pointers to the objects, not references, because the pool
/// does not have the authority to create references to its contents as user code may
/// concurrently be holding a conflicting exclusive reference via `PooledMut<T>`.
///
/// Therefore, obtaining actual references to pool contents via iteration is only possible
/// by using the pointer to create such references in unsafe code and relies on the caller
/// guaranteeing that no conflicting exclusive references exist.
#[derive(Debug)]
pub struct PinnedPoolIterator<'p, T: 'static> {
    raw_iter: RawOpaquePoolIterator<'p>,
    _phantom: std::marker::PhantomData<T>,
}

impl<'p, T> PinnedPoolIterator<'p, T> {
    fn new(pool: &'p RawOpaquePoolSend) -> Self {
        Self {
            raw_iter: pool.iter(),
            _phantom: std::marker::PhantomData,
        }
    }
}

impl<T> Iterator for PinnedPoolIterator<'_, T> {
    type Item = NonNull<T>;

    fn next(&mut self) -> Option<Self::Item> {
        self.raw_iter.next().map(NonNull::cast::<T>)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.raw_iter.size_hint()
    }
}

impl<T> DoubleEndedIterator for PinnedPoolIterator<'_, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.raw_iter.next_back().map(NonNull::cast::<T>)
    }
}

impl<T> ExactSizeIterator for PinnedPoolIterator<'_, T> {
    fn len(&self) -> usize {
        self.raw_iter.len()
    }
}

impl<T> FusedIterator for PinnedPoolIterator<'_, T> {}

#[cfg(test)]
mod tests {
    use std::mem::MaybeUninit;

    use super::*;

    #[test]
    fn new_pool_is_empty() {
        let pool = PinnedPool::<u32>::new();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity(), 0);
    }

    #[test]
    fn default_pool_is_empty() {
        let pool: PinnedPool<String> = PinnedPool::default();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity(), 0);
    }

    #[test]
    fn insert_and_length() {
        let mut pool = PinnedPool::<u64>::new();

        let _h1 = pool.insert(10);
        assert_eq!(pool.len(), 1);
        assert!(!pool.is_empty());

        let _h2 = pool.insert(20);
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn insert_with_closure() {
        let mut pool = PinnedPool::<u64>::new();

        // SAFETY: we correctly initialize the value
        let handle = unsafe {
            pool.insert_with(|u: &mut MaybeUninit<u64>| {
                u.write(42);
            })
        };

        assert_eq!(pool.len(), 1);
        assert_eq!(*handle, 42);
    }

    #[test]
    fn clone_arc_like_behavior() {
        let mut p1 = PinnedPool::<u32>::new();
        let mut p2 = p1.clone();

        let _h1 = p1.insert(100);
        assert_eq!(p2.len(), 1);

        let _h2 = p2.insert(200);
        assert_eq!(p1.len(), 2);

        // Both can iterate and see the same contents
        let mut values1: Vec<u32> = p1.with_iter(|iter| {
            iter.map(|ptr| {
                // SAFETY: The iterator yields valid NonNull<T> pointers for items still in the pool.
                unsafe { *ptr.as_ref() }
            })
            .collect()
        });
        let mut values2: Vec<u32> = p2.with_iter(|iter| {
            iter.map(|ptr| {
                // SAFETY: The iterator yields valid NonNull<T> pointers for items still in the pool.
                unsafe { *ptr.as_ref() }
            })
            .collect()
        });
        values1.sort_unstable();
        values2.sort_unstable();
        assert_eq!(values1, vec![100, 200]);
        assert_eq!(values2, vec![100, 200]);
    }

    #[test]
    fn lifecycle_handles_keep_pool_alive() {
        let handle = {
            let mut pool = PinnedPool::<String>::new();
            pool.insert("persist".to_string())
        }; // pool dropped here

        assert_eq!(&*handle, "persist");

        // Dropping handle now removes from pool (implicitly tested by absence of panic)
        drop(handle);
    }

    #[test]
    fn lifecycle_pool_clone_keeps_inner_alive() {
        let mut pool = PinnedPool::<String>::new();

        // Insert & clone
        let handle = pool.insert("data".to_string());
        let pool_clone = pool.clone();

        // Drop original pool; handle + clone should keep inner alive
        drop(pool); // original handle dropped

        // Validate access still works
        assert_eq!(&*handle, "data");
        assert_eq!(pool_clone.len(), 1);

        // Drop last handle; pool should now be empty
        drop(handle);
        assert_eq!(pool_clone.len(), 0);
    }

    #[test]
    fn pooled_mut_mutation_reflected() {
        let mut pool = PinnedPool::<String>::new();

        let mut handle = pool.insert("hello".to_string());
        handle.push_str(" world");

        assert_eq!(&*handle, "hello world");
        assert_eq!(pool.len(), 1);

        // Iteration sees updated value
        pool.with_iter(|iter| {
            let vals: Vec<String> = iter
                .map(|p| {
                    // SAFETY: Iterator guarantees each pointer is a valid &String for the duration of closure.
                    unsafe { p.as_ref().clone() }
                })
                .collect();
            assert_eq!(vals, vec!["hello world".to_string()]);
        });
    }

    #[test]
    fn reserve_and_shrink_to_fit_shared() {
        let mut p1 = PinnedPool::<u8>::new();
        let mut p2 = p1.clone();

        // Reserve capacity via one handle
        p1.reserve(50);
        assert!(p1.capacity() >= 50);
        assert_eq!(p1.capacity(), p2.capacity());

        // Add and then drop items to create unused capacity
        let h = p2.insert(99);
        assert_eq!(p1.len(), 1);
        drop(h);
        assert_eq!(p2.len(), 0);

        // Shrink and ensure both views updated
        p1.shrink_to_fit();
        assert_eq!(p1.capacity(), p2.capacity());
    }

    #[test]
    fn with_iter_empty_pool() {
        let pool = PinnedPool::<u32>::new();
        let count = pool.with_iter(|iter| iter.count());
        assert_eq!(count, 0);
    }

    #[test]
    fn with_iter_collect_values() {
        let mut pool = PinnedPool::<u32>::new();

        let _handles: Vec<_> = [10, 20, 30].into_iter().map(|v| pool.insert(v)).collect();

        let mut collected: Vec<u32> = pool.with_iter(|iter| {
            iter.map(|p| {
                // SAFETY: Iterator yields valid NonNull<u32> pointers for items alive in pool.
                unsafe { *p.as_ref() }
            })
            .collect()
        });

        collected.sort_unstable();
        assert_eq!(collected, vec![10, 20, 30]);
    }

    #[test]
    fn with_iter_double_ended() {
        let mut pool = PinnedPool::<i32>::new();
        let _handles: Vec<_> = [1, 2, 3].into_iter().map(|v| pool.insert(v)).collect();

        pool.with_iter(|mut iter| {
            assert_eq!(iter.len(), 3);

            let _front = iter.next();
            assert_eq!(iter.len(), 2);

            let _back = iter.next_back();
            assert_eq!(iter.len(), 1);

            let _remaining = iter.next();
            assert!(iter.next().is_none());
            assert!(iter.next_back().is_none());
        });
    }

    #[test]
    fn with_iter_fused_behavior() {
        let pool = PinnedPool::<u32>::new();
        pool.with_iter(|mut iter| {
            assert!(iter.next().is_none());
            assert!(iter.next().is_none());
            assert!(iter.next_back().is_none());
        });
    }
}
