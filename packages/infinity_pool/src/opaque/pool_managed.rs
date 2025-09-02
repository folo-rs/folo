use std::alloc::Layout;
use std::iter::FusedIterator;
use std::mem::MaybeUninit;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use crate::opaque::pool_raw::RawOpaquePoolIterator;
use crate::{ERR_POISONED_LOCK, PooledMut, RawOpaquePool, RawOpaquePoolSend};

/// A pool of reference-counted objects with uniform memory layout.
///
/// Stores objects of any `Send` type that match a [`Layout`] defined at pool creation
/// time. All values in the pool remain pinned for their entire lifetime.
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
/// The pool is thread-mobile (`Send`) and requires that any inserted items are `Send`, as well.
#[derive(Debug)]
pub struct OpaquePool {
    // The pool type itself is just a handle around the inner pool,
    // which is reference-counted and mutex-guarded. The inner pool
    // will only ever be dropped once all items have been removed from
    // it and no more `OpaquePool` instances exist that point to it.
    //
    // This also implies that `DropPolicy` has no meaning for this
    // pool configuration, as the pool can never be dropped if it has
    // contents (as dropping the handles of pooled objects will remove
    // them from the pool, while keeping the pool alive until then).
    inner: Arc<Mutex<RawOpaquePoolSend>>,
}

impl OpaquePool {
    /// Creates a new instance of the pool with the specified layout.
    ///
    /// Shorthand for a builder that keeps all other options at their default values.
    ///
    /// # Panics
    ///
    /// Panics if the layout is zero-sized.
    #[must_use]
    pub fn with_layout(object_layout: Layout) -> Self {
        let inner = RawOpaquePool::with_layout(object_layout);

        // SAFETY: All insertion methods require `T: Send`.
        let inner = unsafe { RawOpaquePoolSend::new(inner) };

        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    /// Creates a new instance of the pool with the layout of `T`.
    ///
    /// Shorthand for a builder that keeps all other options at their default values.
    ///
    /// # Panics
    ///
    /// Panics if `T` is a zero-sized type.
    #[must_use]
    pub fn with_layout_of<T: Sized + Send>() -> Self {
        Self::with_layout(Layout::new::<T>())
    }

    /// The layout of objects stored in this pool.
    #[must_use]
    pub fn object_layout(&self) -> Layout {
        self.inner.lock().expect(ERR_POISONED_LOCK).object_layout()
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

    /// Reserves capacity for at least `additional` more objects.
    ///
    /// The new capacity is calculated from the current `len()`, not from the current capacity.
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
    ///
    /// # Panics
    ///
    /// Panics if the layout of `T` does not match the object layout of the pool.
    pub fn insert<T: Send>(&mut self, value: T) -> PooledMut<T> {
        let inner = self.inner.lock().expect(ERR_POISONED_LOCK).insert(value);

        PooledMut::new(inner, Arc::clone(&self.inner))
    }

    /// Inserts an object into the pool and returns a handle to it.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the layout of `T` matches the pool's object layout.
    pub unsafe fn insert_unchecked<T: Send>(&mut self, value: T) -> PooledMut<T> {
        // SAFETY: Forwarding safety guarantees from caller.
        let inner = unsafe {
            self.inner
                .lock()
                .expect(ERR_POISONED_LOCK)
                .insert_unchecked(value)
        };

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
    /// # Panics
    ///
    /// Panics if the layout of `T` does not match the object layout of the pool.
    ///
    /// # Safety
    ///
    /// The closure must correctly initialize the object. All fields that
    /// are not `MaybeUninit` must be initialized when the closure returns.
    pub unsafe fn insert_with<T, F>(&mut self, f: F) -> PooledMut<T>
    where
        T: Send,
        F: FnOnce(&mut MaybeUninit<T>),
    {
        // SAFETY: Forwarding safety guarantees from caller.
        let inner = unsafe { self.inner.lock().expect(ERR_POISONED_LOCK).insert_with(f) };

        PooledMut::new(inner, Arc::clone(&self.inner))
    }

    /// Inserts an object into the pool via closure and returns a handle to it.
    ///
    /// This method allows the caller to partially initialize the object, skipping any `MaybeUninit`
    /// fields that are intentionally not initialized at insertion time. This can make insertion of
    /// objects containing `MaybeUninit` fields faster, although requires unsafe code to implement.
    ///
    /// This method is NOT faster than `insert_unchecked()` for fully initialized objects.
    /// Prefer `insert_unchecked()` for a better safety posture if you do not intend to
    /// skip initialization of any `MaybeUninit` fields.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the layout of `T` matches the pool's object layout.
    ///
    /// The closure must correctly initialize the object. All fields that
    /// are not `MaybeUninit` must be initialized when the closure returns.
    pub unsafe fn insert_with_unchecked<T, F>(&mut self, f: F) -> PooledMut<T>
    where
        T: Send,
        F: FnOnce(&mut MaybeUninit<T>),
    {
        // SAFETY: Forwarding safety guarantees from caller.
        let inner = unsafe {
            self.inner
                .lock()
                .expect(ERR_POISONED_LOCK)
                .insert_with_unchecked(f)
        };

        PooledMut::new(inner, Arc::clone(&self.inner))
    }

    /// Calls a closure with an iterator over all objects in the pool.
    ///
    /// The iterator yields untyped pointers (`NonNull<()>`) to the objects stored in the pool.
    /// It is the caller's responsibility to cast these pointers to the appropriate type.
    ///
    /// The pool is locked for the entire duration of the closure, ensuring that objects
    /// cannot be removed while iteration is in progress. This guarantees that all pointers
    /// yielded by the iterator remain valid for the duration of the closure.
    ///
    /// # Examples
    ///
    /// ```
    /// # use infinity_pool::OpaquePool;
    /// let mut pool = OpaquePool::with_layout_of::<u32>();
    /// let _handle1 = pool.insert(42u32);
    /// let _handle2 = pool.insert(100u32);
    /// 
    /// // Safe iteration with guaranteed pointer validity
    /// pool.with_iter(|iter| {
    ///     for ptr in iter {
    ///         // SAFETY: We know these are u32 pointers from this pool
    ///         let value = unsafe { *ptr.cast::<u32>().as_ref() };
    ///         println!("Value: {}", value);
    ///     }
    /// });
    /// 
    /// // Collect values safely
    /// let values: Vec<u32> = pool.with_iter(|iter| {
    ///     iter.map(|ptr| unsafe { *ptr.cast::<u32>().as_ref() }).collect()
    /// });
    /// ```
    ///
    pub fn with_iter<F, R>(&self, f: F) -> R
    where
        F: FnOnce(OpaquePoolIterator<'_>) -> R,
    {
        let guard = self.inner.lock().expect(ERR_POISONED_LOCK);
        let iter = OpaquePoolIterator::new(&guard);
        f(iter)
    }
}

/// Iterator over all objects in an opaque pool.
///
/// This iterator yields untyped pointers to objects stored in the pool.
/// Since the pool can contain objects of different types (as long as they have the same layout),
/// the iterator returns `NonNull<()>` and leaves type casting to the caller.
///
/// The iterator holds a reference to the locked pool, ensuring that objects cannot be
/// removed while iteration is in progress and that all yielded pointers remain valid.
///
/// # Safety
///
/// While the iterator ensures objects cannot be removed from the pool during iteration,
/// it is still the caller's responsibility to cast the `NonNull<()>` pointers to the
/// correct type that matches the pool's layout.
#[derive(Debug)]
pub struct OpaquePoolIterator<'p> {
    raw_iter: RawOpaquePoolIterator<'p>,
}

impl<'p> OpaquePoolIterator<'p> {
    fn new(pool: &'p RawOpaquePoolSend) -> Self {
        Self {
            raw_iter: pool.iter(),
        }
    }
}

impl Iterator for OpaquePoolIterator<'_> {
    type Item = NonNull<()>;

    fn next(&mut self) -> Option<Self::Item> {
        self.raw_iter.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.raw_iter.size_hint()
    }
}

impl DoubleEndedIterator for OpaquePoolIterator<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.raw_iter.next_back()
    }
}

impl ExactSizeIterator for OpaquePoolIterator<'_> {
    fn len(&self) -> usize {
        self.raw_iter.len()
    }
}

impl FusedIterator for OpaquePoolIterator<'_> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iter_empty_pool() {
        let pool = OpaquePool::with_layout_of::<u32>();
        
        pool.with_iter(|mut iter| {
            assert_eq!(iter.size_hint(), (0, Some(0)));
            assert_eq!(iter.len(), 0);
            
            assert_eq!(iter.next(), None);
            assert_eq!(iter.size_hint(), (0, Some(0)));
            assert_eq!(iter.len(), 0);
        });
    }    #[test]
    fn iter_single_item() {
        let mut pool = OpaquePool::with_layout_of::<u32>();
        
        let _handle = pool.insert(42_u32);
        
        pool.with_iter(|mut iter| {
            assert_eq!(iter.len(), 1);
            
            // First item should be the object we inserted
            let ptr = iter.next().expect("should have one item");
            
            // SAFETY: We know this points to a u32 we just inserted
            let value = unsafe { ptr.cast::<u32>().as_ref() };
            assert_eq!(*value, 42);
            
            // No more items
            assert_eq!(iter.next(), None);
            assert_eq!(iter.len(), 0);
        });
    }    #[test]
    fn iter_multiple_items() {
        let mut pool = OpaquePool::with_layout_of::<u32>();
        
        let _handle1 = pool.insert(100_u32);
        let _handle2 = pool.insert(200_u32);
        let _handle3 = pool.insert(300_u32);
        
        pool.with_iter(|iter| {
            let values: Vec<u32> = iter
                .map(|ptr| {
                    // SAFETY: We know these point to u32s we inserted
                    unsafe { *ptr.cast::<u32>().as_ref() }
                })
                .collect();
            
            assert_eq!(values, vec![100, 200, 300]);
        });
    }    #[test]
    fn iter_double_ended_basic() {
        let mut pool = OpaquePool::with_layout_of::<u32>();
        
        let _handle1 = pool.insert(100_u32);
        let _handle2 = pool.insert(200_u32);
        let _handle3 = pool.insert(300_u32);
        
        pool.with_iter(|mut iter| {
            // Iterate from the back
            let last_ptr = iter.next_back().expect("should have last item");
            // SAFETY: We know this points to a u32 we inserted
            let last_value = unsafe { *last_ptr.cast::<u32>().as_ref() };
            assert_eq!(last_value, 300);
            
            let middle_ptr = iter.next_back().expect("should have middle item");
            // SAFETY: We know this points to a u32 we inserted
            let middle_value = unsafe { *middle_ptr.cast::<u32>().as_ref() };
            assert_eq!(middle_value, 200);
            
            let first_ptr = iter.next().expect("should have first item");
            // SAFETY: We know this points to a u32 we inserted
            let first_value = unsafe { *first_ptr.cast::<u32>().as_ref() };
            assert_eq!(first_value, 100);
            
            // Should be exhausted now
            assert_eq!(iter.next(), None);
            assert_eq!(iter.next_back(), None);
        });
    }    #[test]
    fn with_iter_scoped_access() {
        let mut pool = OpaquePool::with_layout_of::<u32>();
        
        let _handle1 = pool.insert(100_u32);
        let _handle2 = pool.insert(200_u32);
        let _handle3 = pool.insert(300_u32);
        
        // Test that we can use the iterator in a scoped manner
        let result = pool.with_iter(|iter| {
            let mut values = Vec::new();
            for ptr in iter {
                // SAFETY: We know these point to u32s we just inserted
                let value = unsafe { *ptr.cast::<u32>().as_ref() };
                values.push(value);
            }
            values
        });
        
        assert_eq!(result, vec![100, 200, 300]);
    }    #[test]
    fn with_iter_holds_lock() {
        let mut pool = OpaquePool::with_layout_of::<u32>();
        
        let _handle1 = pool.insert(100_u32);
        let _handle2 = pool.insert(200_u32);
        
        // Test that iteration sees a consistent view
        pool.with_iter(|iter| {
            assert_eq!(iter.len(), 2);
            
            let values: Vec<u32> = iter
                .map(|ptr| {
                    // SAFETY: We know these point to u32s we inserted
                    unsafe { *ptr.cast::<u32>().as_ref() }
                })
                .collect();
            
            assert_eq!(values, vec![100, 200]);
        });
        
        // After the scope, we can modify the pool again
        let _handle3 = pool.insert(300_u32);
        
        // A new with_iter call should see all 3 items
        pool.with_iter(|iter| {
            assert_eq!(iter.len(), 3);
        });
    }
}
