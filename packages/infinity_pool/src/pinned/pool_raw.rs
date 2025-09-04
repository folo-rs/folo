use std::fmt;
use std::iter::FusedIterator;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::ptr::NonNull;

use crate::{
    DropPolicy, RawOpaquePool, RawOpaquePoolIterator, RawPinnedPoolBuilder, RawPooled, RawPooledMut,
};

/// A pool of objects of type `T`.
///
/// All values in the pool remain pinned for their entire lifetime.
///
/// The pool automatically expands its capacity when needed.
///
/// # Thread safety
///
/// If `T: Send` then the pool is thread-mobile (`Send` but not `Sync`).
///
/// If `T: !Send`, the pool is single-threaded.
pub struct RawPinnedPool<T> {
    /// The underlying pool that manages memory and storage.
    inner: RawOpaquePool,

    /// Phantom data to associate the pool with type T.
    _marker: PhantomData<T>,
}

impl<T> RawPinnedPool<T> {
    /// Starts configuring and creating a new instance of the pool.
    #[cfg_attr(test, mutants::skip)] // Gets mutated to alternate version of itself.
    pub fn builder() -> RawPinnedPoolBuilder<T> {
        RawPinnedPoolBuilder::new()
    }

    /// Creates a new pool with the default configuration.
    #[must_use]
    pub fn new() -> Self {
        Self::new_inner(DropPolicy::default())
    }

    #[must_use]
    pub(crate) fn new_inner(drop_policy: DropPolicy) -> Self {
        let inner = RawOpaquePool::builder()
            .layout_of::<T>()
            .drop_policy(drop_policy)
            .build();

        Self {
            inner,
            _marker: PhantomData,
        }
    }

    /// The number of objects currently in the pool.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// The total capacity of the pool.
    ///
    /// This is the maximum number of objects that the pool can contain without capacity extension.
    /// The pool will automatically extend its capacity if more than this many objects are inserted.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    /// Whether the pool contains zero objects.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Ensures that the pool has capacity for at least `additional` more objects.
    ///
    /// # Panics
    ///
    /// Panics if the new capacity would exceed the size of virtual memory.
    pub fn reserve(&mut self, additional: usize) {
        self.inner.reserve(additional);
    }

    /// Drops unused pool capacity to reduce memory usage.
    ///
    /// There is no guarantee that any unused capacity can be dropped. The exact outcome depends
    /// on the specific pool structure and which objects remain in the pool.
    pub fn shrink_to_fit(&mut self) {
        self.inner.shrink_to_fit();
    }

    /// Inserts an object into the pool and returns a handle to it.
    pub fn insert(&mut self, value: T) -> RawPooledMut<T> {
        // SAFETY: match between T and inner pool layout is a type invariant.
        unsafe { self.inner.insert_unchecked(value) }
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
    pub unsafe fn insert_with<F>(&mut self, f: F) -> RawPooledMut<T>
    where
        F: FnOnce(&mut MaybeUninit<T>),
    {
        // SAFETY: match between T and inner pool layout is a type invariant.
        // Completeness of initialization is a guarantee forwarded from the caller.
        unsafe { self.inner.insert_with_unchecked(f) }
    }

    /// Removes an object from the pool, dropping it.
    ///
    /// # Panics
    ///
    /// Panics if the handle does not reference an object in this pool.
    pub fn remove_mut<P: ?Sized>(&mut self, handle: RawPooledMut<P>) {
        self.inner.remove_mut(handle);
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
    pub unsafe fn remove<P: ?Sized>(&mut self, handle: RawPooled<P>) {
        // SAFETY: Forwarding safety guarantees from the caller.
        unsafe {
            self.inner.remove(handle);
        }
    }

    /// Returns an iterator over all objects in the pool.
    #[must_use]
    pub fn iter(&self) -> RawPinnedPoolIterator<'_, T> {
        RawPinnedPoolIterator::new(self)
    }
}

impl<T> RawPinnedPool<T>
where
    T: Unpin,
{
    /// Removes an object from the pool and returns it.
    ///
    /// # Panics
    ///
    /// Panics if the handle does not reference an object in this pool.
    #[must_use]
    pub fn remove_mut_unpin(&mut self, handle: RawPooledMut<T>) -> T {
        self.inner.remove_mut_unpin(handle)
    }

    /// Removes an object from the pool and returns it.
    ///
    /// # Panics
    ///
    /// Panics if the handle does not reference an existing object in this pool.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the handle belongs to this pool and that the object it
    /// references has not already been removed from the pool.
    #[must_use]
    pub unsafe fn remove_unpin(&mut self, handle: RawPooled<T>) -> T {
        // SAFETY: Forwarding safety guarantees from the caller.
        unsafe { self.inner.remove_unpin(handle) }
    }
}

impl<T> Default for RawPinnedPool<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> fmt::Debug for RawPinnedPool<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawPinnedPool")
            .field("inner", &self.inner)
            .finish()
    }
}

// SAFETY: RawPinnedPool<T> is Send when T is Send. This is possible because the underlying
// RawOpaquePool allows us to consider it `Send` when all objects inserted into it are `Send`,
// which we guarantee via the type parameter T.
unsafe impl<T> Send for RawPinnedPool<T>
where
    T: Send,
{
}

/// Iterator over all objects in a [`RawPinnedPool`].
///
/// The iterator only yields pointers to the objects, not references, because the pool
/// does not have the authority to create references to its contents as user code may
/// concurrently be holding a conflicting exclusive reference via `RawPooledMut<T>`.
///
/// Therefore, obtaining actual references to pool contents via iteration is only possible
/// by using the pointer to create such references in unsafe code and relies on the caller
/// guaranteeing that no conflicting exclusive references exist.
///
/// # Thread safety
///
/// The type is single-threaded.
#[derive(Debug)]
pub struct RawPinnedPoolIterator<'p, T> {
    inner: RawOpaquePoolIterator<'p>,
    _marker: PhantomData<&'p T>,
}

impl<'p, T> RawPinnedPoolIterator<'p, T> {
    fn new(pool: &'p RawPinnedPool<T>) -> Self {
        Self {
            inner: pool.inner.iter(),
            _marker: PhantomData,
        }
    }
}

impl<T> Iterator for RawPinnedPoolIterator<'_, T> {
    type Item = NonNull<T>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(NonNull::cast::<T>)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<T> DoubleEndedIterator for RawPinnedPoolIterator<'_, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.inner.next_back().map(NonNull::cast::<T>)
    }
}

impl<T> ExactSizeIterator for RawPinnedPoolIterator<'_, T> {
    fn len(&self) -> usize {
        self.inner.len()
    }
}

impl<T> FusedIterator for RawPinnedPoolIterator<'_, T> {}

impl<'p, T> IntoIterator for &'p RawPinnedPool<T> {
    type Item = NonNull<T>;
    type IntoIter = RawPinnedPoolIterator<'p, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use std::mem::MaybeUninit;
    use std::rc::Rc;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    // When T: Send, the pool should be Send but not Sync
    assert_impl_all!(RawPinnedPool<i32>: Send);
    assert_not_impl_any!(RawPinnedPool<i32>: Sync);

    // When T: !Send, the pool should be neither Send nor Sync
    assert_not_impl_any!(RawPinnedPool<Rc<i32>>: Send, Sync);

    // Iterator trait assertions
    assert_impl_all!(RawPinnedPoolIterator<'_, i32>: Iterator, DoubleEndedIterator, ExactSizeIterator, FusedIterator);
    assert_not_impl_any!(RawPinnedPoolIterator<'_, i32>: Send, Sync);
    assert_impl_all!(&RawPinnedPool<i32>: IntoIterator);

    #[test]
    fn new_pool_is_empty() {
        let pool = RawPinnedPool::<u64>::new();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity(), 0);
    }

    #[test]
    fn default_pool_is_empty() {
        let pool = RawPinnedPool::<String>::default();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity(), 0);
    }

    #[test]
    fn instance_creation_through_builder_succeeds() {
        let pool = RawPinnedPool::<String>::builder().build();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity(), 0);
    }

    #[test]
    fn insert_and_length() {
        let mut pool = RawPinnedPool::<u32>::new();

        let _handle1 = pool.insert(42_u32);
        assert_eq!(pool.len(), 1);
        assert!(!pool.is_empty());

        let _handle2 = pool.insert(100_u32);
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn capacity_grows_when_needed() {
        let mut pool = RawPinnedPool::<u64>::new();

        assert_eq!(pool.capacity(), 0);

        let _handle = pool.insert(123_u64);

        // Should have some capacity now
        assert!(pool.capacity() > 0);
        let initial_capacity = pool.capacity();

        // Fill up the pool to force capacity expansion
        #[expect(
            clippy::collection_is_never_read,
            reason = "handles are used for ownership"
        )]
        let mut handles = Vec::new();
        for i in 1..initial_capacity {
            handles.push(pool.insert(i as u64));
        }

        // One more insert should expand capacity
        let _handle = pool.insert(999_u64);

        assert!(pool.capacity() >= initial_capacity * 2);
    }

    #[test]
    fn reserve_creates_capacity() {
        let mut pool = RawPinnedPool::<u8>::new();

        pool.reserve(100);
        assert!(pool.capacity() >= 100);

        let initial_capacity = pool.capacity();
        pool.reserve(50); // Should not increase capacity
        assert_eq!(pool.capacity(), initial_capacity);

        pool.reserve(200); // Should increase capacity
        assert!(pool.capacity() >= 200);
    }

    #[test]
    fn insert_with_closure() {
        let mut pool = RawPinnedPool::<u64>::new();

        // SAFETY: we correctly initialize the slot.
        let handle = unsafe {
            pool.insert_with(|uninit: &mut MaybeUninit<u64>| {
                uninit.write(42);
            })
        };

        assert_eq!(pool.len(), 1);
        assert_eq!(*handle, 42);
    }

    #[test]
    fn remove_decreases_length() {
        let mut pool = RawPinnedPool::<String>::new();

        let handle1 = pool.insert("hello".to_string());
        let handle2 = pool.insert("world".to_string());

        assert_eq!(pool.len(), 2);

        pool.remove_mut(handle1);
        assert_eq!(pool.len(), 1);

        pool.remove_mut(handle2);
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn remove_with_shared_handle() {
        let mut pool = RawPinnedPool::<i64>::new();

        let handle = pool.insert(999_i64).into_shared();

        assert_eq!(pool.len(), 1);

        // SAFETY: Handle is from this pool and has not yet been used to remove.
        unsafe {
            pool.remove(handle);
        }

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn shrink_to_fit_removes_unused_capacity() {
        let mut pool = RawPinnedPool::<u8>::new();

        // Add some items to force capacity allocation
        let mut handles = Vec::new();
        for i in 0..10 {
            handles.push(pool.insert(u8::try_from(i).unwrap()));
        }

        // Remove all items
        for handle in handles {
            pool.remove_mut(handle);
        }

        assert!(pool.is_empty());

        pool.shrink_to_fit();

        // With zero items, all capacity should be dropped
        assert_eq!(pool.capacity(), 0);
    }

    #[test]
    fn handle_provides_access_to_object() {
        let mut pool = RawPinnedPool::<u64>::new();

        let handle = pool.insert(12345_u64);

        assert_eq!(*handle, 12345);

        // Verify we can get a pointer to the object
        let ptr = handle.ptr();

        // SAFETY: Handle is valid and points to a u64
        let value = unsafe { ptr.as_ref() };

        assert_eq!(*value, 12345);
    }

    #[test]
    fn multiple_insertions_and_removals() {
        let mut pool = RawPinnedPool::<usize>::new();

        // Test slot reuse
        let handle1 = pool.insert(1_usize);
        pool.remove_mut(handle1);

        let handle2 = pool.insert(2_usize);

        assert_eq!(pool.len(), 1);
        assert_eq!(*handle2, 2);

        pool.remove_mut(handle2);
        assert!(pool.is_empty());
    }

    #[test]
    fn unpin_remove_returns_value() {
        let mut pool = RawPinnedPool::<i32>::new();

        let handle = pool.insert(-456_i32);

        let value = pool.remove_mut_unpin(handle);
        assert_eq!(value, -456);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn unpin_remove_with_shared_handle() {
        let mut pool = RawPinnedPool::<i32>::new();

        let handle = pool.insert(42_i32).into_shared();

        assert_eq!(pool.len(), 1);

        // SAFETY: Handle is from this pool and has not yet been used to remove.
        let value = unsafe { pool.remove_unpin(handle) };

        assert_eq!(value, 42);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn iter_empty_pool() {
        let pool = RawPinnedPool::<u64>::new();
        let mut iter = pool.iter();

        assert_eq!(iter.len(), 0);
        assert_eq!(iter.size_hint(), (0, Some(0)));
        assert!(iter.next().is_none());
        assert!(iter.next_back().is_none());
    }

    #[test]
    fn iter_single_item() {
        let mut pool = RawPinnedPool::<i32>::new();
        let _handle = pool.insert(42);

        let mut iter = pool.iter();
        assert_eq!(iter.len(), 1);
        assert_eq!(iter.size_hint(), (1, Some(1)));

        let ptr = iter.next().expect("should have one item");
        // SAFETY: We know this points to a valid i32 from our insertion
        let value = unsafe { ptr.as_ref() };
        assert_eq!(*value, 42);

        assert_eq!(iter.len(), 0);
        assert!(iter.next().is_none());
    }

    #[test]
    fn iter_multiple_items() {
        let mut pool = RawPinnedPool::<u8>::new();
        let values = [1_u8, 2, 3, 4, 5];
        let _handles: Vec<_> = values.iter().map(|&v| pool.insert(v)).collect();

        let iter = pool.iter();
        assert_eq!(iter.len(), 5);

        let collected_values: Vec<u8> = iter
            // SAFETY: All pointers are valid u8 values from our insertions above
            .map(|ptr| unsafe { *ptr.as_ref() })
            .collect();

        // The iteration order may not match insertion order, so we just verify the count and contents
        assert_eq!(collected_values.len(), 5);
        for expected in values {
            assert!(collected_values.contains(&expected));
        }
    }

    #[test]
    fn iter_double_ended() {
        let mut pool = RawPinnedPool::<u16>::new();
        let _handles: Vec<_> = (0..3_u16).map(|v| pool.insert(v)).collect();

        let mut iter = pool.iter();
        assert_eq!(iter.len(), 3);

        // Take one from front
        let _front = iter.next().expect("should have front item");
        assert_eq!(iter.len(), 2);

        // Take one from back
        let _back = iter.next_back().expect("should have back item");
        assert_eq!(iter.len(), 1);

        // Take remaining
        let _remaining = iter.next().expect("should have remaining item");
        assert_eq!(iter.len(), 0);
        assert!(iter.next().is_none());
        assert!(iter.next_back().is_none());
    }

    #[test]
    fn iter_into_iterator_trait() {
        let mut pool = RawPinnedPool::<char>::new();
        let _handle = pool.insert('x');

        let count = (&pool).into_iter().count();
        assert_eq!(count, 1);
    }

    #[test]
    fn iter_fused() {
        let pool = RawPinnedPool::<u32>::new();
        let mut iter = pool.iter();

        // Empty iterator should keep returning None
        assert!(iter.next().is_none());
        assert!(iter.next().is_none());
        assert!(iter.next_back().is_none());
        assert!(iter.next_back().is_none());
    }
}
