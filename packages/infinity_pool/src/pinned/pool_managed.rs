use std::fmt;
use std::iter::FusedIterator;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::ptr::NonNull;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::{PooledMut, RawOpaquePool, RawOpaquePoolIterator, RawOpaquePoolSend};

/// A thread-safe pool of reference-counted objects of type `T`.
///
/// All values in the pool remain pinned for their entire lifetime.
///
/// The pool automatically expands its capacity when needed.
#[doc = include_str!("../../doc/snippets/managed_pool_lifetimes.md")]
///
/// # Thread safety
///
/// The pool is thread-safe.
///
/// # Example
///
/// ```rust
/// use infinity_pool::PinnedPool;
///
/// let mut pool = PinnedPool::<String>::new();
///
/// // Insert an object into the pool
/// let handle = pool.insert("Hello, Pinned!".to_string());
///
/// // Access the object through the handle
/// assert_eq!(*handle, "Hello, Pinned!");
///
/// // The object is automatically removed when the handle is dropped
/// ```
///
/// # Pool clones are functionally equivalent
///
/// ```rust
/// use infinity_pool::PinnedPool;
///
/// let mut pool1 = PinnedPool::<i32>::new();
/// let pool2 = pool1.clone();
///
/// assert_eq!(pool1.len(), pool2.len());
/// let _handle = pool1.insert(42);
/// assert_eq!(pool1.len(), pool2.len());
/// ```
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

    _phantom: PhantomData<T>,
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
            _phantom: PhantomData,
        }
    }

    #[doc = include_str!("../../doc/snippets/pool_len.md")]
    #[must_use]
    #[inline]
    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }

    #[doc = include_str!("../../doc/snippets/pool_capacity.md")]
    #[must_use]
    #[inline]
    pub fn capacity(&self) -> usize {
        self.inner.lock().capacity()
    }

    #[doc = include_str!("../../doc/snippets/pool_is_empty.md")]
    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.inner.lock().is_empty()
    }

    #[doc = include_str!("../../doc/snippets/pool_reserve.md")]
    #[inline]
    pub fn reserve(&self, additional: usize) {
        self.inner.lock().reserve(additional);
    }

    #[doc = include_str!("../../doc/snippets/pool_shrink_to_fit.md")]
    #[inline]
    pub fn shrink_to_fit(&self) {
        self.inner.lock().shrink_to_fit();
    }

    #[doc = include_str!("../../doc/snippets/pool_insert.md")]
    ///
    /// # Example
    ///
    /// ```rust
    /// use infinity_pool::PinnedPool;
    ///
    /// let mut pool = PinnedPool::<String>::new();
    ///
    /// // Insert an object into the pool
    /// let mut handle = pool.insert("Hello".to_string());
    ///
    /// // Mutate the object via the unique handle
    /// handle.push_str(", World!");
    /// assert_eq!(&*handle, "Hello, World!");
    ///
    /// // Transform the unique handle into a shared handle
    /// let shared_handle = handle.into_shared();
    ///
    /// // After transformation, you can only immutably dereference the object
    /// assert_eq!(&*shared_handle, "Hello, World!");
    /// // shared_handle.push_str("!"); // This would not compile
    ///
    /// // The object is removed when the handle is dropped
    /// drop(shared_handle); // Explicitly drop to remove from pool
    /// assert_eq!(pool.len(), 0);
    /// ```
    #[inline]
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // All mutations are unviable - skip them to save time.
    pub fn insert(&self, value: T) -> PooledMut<T> {
        let inner = self.inner.lock().insert(value);

        PooledMut::new(inner, Arc::clone(&self.inner))
    }

    #[doc = include_str!("../../doc/snippets/pool_insert_with.md")]
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::mem::MaybeUninit;
    ///
    /// use infinity_pool::PinnedPool;
    ///
    /// struct DataBuffer {
    ///     id: u32,
    ///     data: MaybeUninit<[u8; 1024]>, // Large buffer to skip initializing
    /// }
    ///
    /// let mut pool = PinnedPool::<DataBuffer>::new();
    ///
    /// // Initialize only the id, leaving data uninitialized for performance
    /// let handle = unsafe {
    ///     pool.insert_with(|uninit: &mut MaybeUninit<DataBuffer>| {
    ///         let ptr = uninit.as_mut_ptr();
    ///         // SAFETY: Writing to the id field within allocated space
    ///         unsafe {
    ///             std::ptr::addr_of_mut!((*ptr).id).write(42);
    ///             // data field is intentionally left uninitialized
    ///         }
    ///     })
    /// };
    ///
    /// // ID is accessible, data remains uninitialized
    /// let id = unsafe { std::ptr::addr_of!((*handle).id).read() };
    /// assert_eq!(id, 42);
    /// ```
    ///
    /// # Safety
    #[doc = include_str!("../../doc/snippets/safety_closure_must_initialize_object.md")]
    #[inline]
    #[must_use]
    pub unsafe fn insert_with<F>(&self, f: F) -> PooledMut<T>
    where
        F: FnOnce(&mut MaybeUninit<T>),
    {
        // SAFETY: Forwarding safety guarantees from caller.
        let inner = unsafe { self.inner.lock().insert_with(f) };

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
        let guard = self.inner.lock();
        let iter = PinnedPoolIterator::new(&guard);
        f(iter)
    }
}

impl<T> Clone for PinnedPool<T>
where
    T: Send,
{
    #[inline]
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            _phantom: PhantomData,
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
    _phantom: PhantomData<T>,
}

impl<'p, T> PinnedPoolIterator<'p, T> {
    fn new(pool: &'p RawOpaquePoolSend) -> Self {
        Self {
            raw_iter: pool.iter(),
            _phantom: PhantomData,
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
        let pool = PinnedPool::<u64>::new();

        let _h1 = pool.insert(10);
        assert_eq!(pool.len(), 1);
        assert!(!pool.is_empty());

        let _h2 = pool.insert(20);
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn capacity_grows_when_needed() {
        let pool = PinnedPool::<u64>::new();

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

        assert!(pool.capacity() >= initial_capacity);
    }

    #[test]
    fn reserve_creates_capacity() {
        let pool = PinnedPool::<u8>::new();

        pool.reserve(100);
        assert!(pool.capacity() >= 100);

        let initial_capacity = pool.capacity();
        pool.reserve(50); // Should not increase capacity
        assert_eq!(pool.capacity(), initial_capacity);

        pool.reserve(200); // Should increase capacity
        assert!(pool.capacity() >= 200);
    }

    #[test]
    fn shrink_to_fit_removes_unused_capacity() {
        let pool = PinnedPool::<u8>::new();

        // Reserve more than we need
        pool.reserve(100);

        // Insert only a few items
        let _handle1 = pool.insert(1_u8);
        let _handle2 = pool.insert(2_u8);

        // Shrink should not panic
        pool.shrink_to_fit();

        // Pool should still work normally
        assert_eq!(pool.len(), 2);
        let _handle3 = pool.insert(3_u8);
        assert_eq!(pool.len(), 3);
    }

    #[test]
    fn shrink_to_fit_with_zero_items_shrinks_to_zero_capacity() {
        let pool = PinnedPool::<u8>::new();

        // Add some items to create capacity
        let handle1 = pool.insert(1_u8);
        let handle2 = pool.insert(2_u8);
        let handle3 = pool.insert(3_u8);

        // Verify we have capacity
        assert!(pool.capacity() > 0);

        // Remove all items by dropping handles
        drop(handle1);
        drop(handle2);
        drop(handle3);

        assert!(pool.is_empty());

        pool.shrink_to_fit();

        // Testing implementation detail: empty pool should shrink capacity to zero
        // This may become untrue with future algorithm changes, at which point
        // we will need to adjust the tests.
        assert_eq!(pool.capacity(), 0);
    }

    #[test]
    fn handle_provides_access_to_object() {
        let pool = PinnedPool::<u64>::new();

        let handle = pool.insert(12345_u64);

        assert_eq!(*handle, 12345);
    }

    #[test]
    fn multiple_handles_to_same_type() {
        let pool = PinnedPool::<String>::new();

        let handle1 = pool.insert("hello".to_string());
        let handle2 = pool.insert("world".to_string());

        assert_eq!(pool.len(), 2);

        assert_eq!(&*handle1, "hello");
        assert_eq!(&*handle2, "world");

        // Dropping handles should remove items from pool
        drop(handle1);
        assert_eq!(pool.len(), 1);

        drop(handle2);
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn handle_drop_removes_objects_both_exclusive_and_shared() {
        let pool = PinnedPool::<String>::new();

        // Test exclusive handle drop
        let exclusive_handle = pool.insert("exclusive".to_string());
        assert_eq!(pool.len(), 1);
        drop(exclusive_handle);
        assert_eq!(pool.len(), 0);

        // Test shared handle drop
        let mut_handle = pool.insert("shared".to_string());
        let shared_handle = mut_handle.into_shared();
        assert_eq!(pool.len(), 1);

        // Verify shared handle works
        assert_eq!(&*shared_handle, "shared");

        // Drop the shared handle should remove from pool
        drop(shared_handle);
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn insert_with_closure() {
        let pool = PinnedPool::<u64>::new();

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
    fn clone_behavior() {
        let p1 = PinnedPool::<u32>::new();
        let p2 = p1.clone();

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
            let pool = PinnedPool::<String>::new();
            pool.insert("persist".to_string())
        }; // pool dropped here

        assert_eq!(&*handle, "persist");

        // Dropping handle now removes from pool (implicitly tested by absence of panic)
        drop(handle);
    }

    #[test]
    fn lifecycle_pool_clone_keeps_inner_alive() {
        let pool = PinnedPool::<String>::new();

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
        let pool = PinnedPool::<String>::new();

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
        let p1 = PinnedPool::<u8>::new();
        let p2 = p1.clone();

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
        let pool = PinnedPool::<u32>::new();

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
        let pool = PinnedPool::<i32>::new();
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

    #[test]
    fn iter_size_hint_and_exact_size() {
        let pool = PinnedPool::<u32>::new();

        // Empty pool
        pool.with_iter(|iter| {
            assert_eq!(iter.size_hint(), (0, Some(0)));
            assert_eq!(iter.len(), 0);
        });

        // Add some items
        let _handle1 = pool.insert(100_u32);
        let _handle2 = pool.insert(200_u32);

        pool.with_iter(|mut iter| {
            assert_eq!(iter.size_hint(), (2, Some(2)));
            assert_eq!(iter.len(), 2);

            // Consume one item
            let first_item = iter.next();
            assert!(first_item.is_some());
            assert_eq!(iter.size_hint(), (1, Some(1)));
            assert_eq!(iter.len(), 1);

            // Consume another
            let second_item = iter.next();
            assert!(second_item.is_some());
            assert_eq!(iter.size_hint(), (0, Some(0)));
            assert_eq!(iter.len(), 0);

            // Should be exhausted now
            assert_eq!(iter.next(), None);
            assert_eq!(iter.size_hint(), (0, Some(0)));
            assert_eq!(iter.len(), 0);
        });
    }

    #[test]
    fn thread_safety() {
        use std::sync::Arc;
        use std::thread;

        let pool = PinnedPool::<String>::new();

        // Insert some initial data
        let handle1 = pool.insert("Thread test 1".to_string());
        let handle2 = pool.insert("Thread test 2".to_string());

        let pool = Arc::new(pool);
        let pool_clone = Arc::clone(&pool);

        // Spawn a thread that can access the pool
        let thread_handle = thread::spawn(move || {
            // Should be able to read the length
            assert!(pool_clone.len() >= 2);

            // Should be able to check capacity
            assert!(pool_clone.capacity() >= 2);

            // Should be able to check if empty
            assert!(!pool_clone.is_empty());
        });

        // Wait for thread to complete
        thread_handle.join().unwrap();

        // Original handles should still be valid
        assert_eq!(&*handle1, "Thread test 1");
        assert_eq!(&*handle2, "Thread test 2");
    }
}
