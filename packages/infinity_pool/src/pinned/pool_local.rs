use std::cell::RefCell;
use std::fmt;
use std::iter::FusedIterator;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::ptr::NonNull;
use std::rc::Rc;

use crate::{LocalPooledMut, RawOpaquePool, RawOpaquePoolIterator};

/// A single-threaded pool of reference-counted objects of type `T`.
///
/// All values in the pool remain pinned for their entire lifetime.
///
/// The pool automatically expands its capacity when needed.
#[doc = include_str!("../../doc/snippets/local_pool_lifetimes.md")]
///
/// # Thread safety
///
/// The pool is single-threaded.
pub struct LocalPinnedPool<T: 'static> {
    // We require 'static from any inserted values because the pool
    // does not enforce any Rust lifetime semantics, only reference counts.
    //
    // The pool type itself is just a handle around the inner opaque pool,
    // which is reference-counted and refcell-guarded. The inner pool
    // will only ever be dropped once all items have been removed from
    // it and no more `LocalPinnedPool` instances exist that point to it.
    //
    // This also implies that `DropPolicy` has no meaning for this
    // pool configuration, as the pool can never be dropped if it has
    // contents (as dropping the handles of pooled objects will remove
    // them from the pool, while keeping the pool alive until then).
    inner: Rc<RefCell<RawOpaquePool>>,

    _phantom: PhantomData<T>,
}

impl<T> LocalPinnedPool<T>
where
    T: 'static,
{
    /// Creates a new pool for objects of type `T`.
    #[must_use]
    pub fn new() -> Self {
        let inner = RawOpaquePool::with_layout_of::<T>();

        Self {
            inner: Rc::new(RefCell::new(inner)),
            _phantom: PhantomData,
        }
    }

    #[doc = include_str!("../../doc/snippets/pool_len.md")]
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.borrow().len()
    }

    #[doc = include_str!("../../doc/snippets/pool_capacity.md")]
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.inner.borrow().capacity()
    }

    #[doc = include_str!("../../doc/snippets/pool_is_empty.md")]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.borrow().is_empty()
    }

    #[doc = include_str!("../../doc/snippets/pool_reserve.md")]
    pub fn reserve(&mut self, additional: usize) {
        self.inner.borrow_mut().reserve(additional);
    }

    #[doc = include_str!("../../doc/snippets/pool_shrink_to_fit.md")]
    pub fn shrink_to_fit(&mut self) {
        self.inner.borrow_mut().shrink_to_fit();
    }

    #[doc = include_str!("../../doc/snippets/pool_insert.md")]
    pub fn insert(&mut self, value: T) -> LocalPooledMut<T> {
        let inner = self.inner.borrow_mut().insert(value);

        LocalPooledMut::new(inner, Rc::clone(&self.inner))
    }

    #[doc = include_str!("../../doc/snippets/pool_insert_with.md")]
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::mem::MaybeUninit;
    /// use infinity_pool::LocalPinnedPool;
    ///
    /// struct DataBuffer {
    ///     id: u32,
    ///     data: MaybeUninit<[u8; 1024]>, // Large buffer to skip initializing
    /// }
    ///
    /// let mut pool = LocalPinnedPool::<DataBuffer>::new();
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
    pub unsafe fn insert_with<F>(&mut self, f: F) -> LocalPooledMut<T>
    where
        F: FnOnce(&mut MaybeUninit<T>),
    {
        // SAFETY: Forwarding safety guarantees from caller.
        let inner = unsafe { self.inner.borrow_mut().insert_with(f) };

        LocalPooledMut::new(inner, Rc::clone(&self.inner))
    }

    /// Calls a closure with an iterator over all objects in the pool.
    ///
    /// The iterator only yields pointers to the objects, not references, because the pool
    /// does not have the authority to create references to its contents as user code may
    /// concurrently be holding a conflicting exclusive reference via `LocalPooledMut<T>`.
    ///
    /// Therefore, obtaining actual references to pool contents via iteration is only possible
    /// by using the pointer to create such references in unsafe code and relies on the caller
    /// guaranteeing that no conflicting exclusive references exist.
    ///
    /// # Mutual exclusion
    ///
    /// The pool is borrowed for the entire duration of the closure, ensuring that objects
    /// cannot be removed while iteration is in progress. This guarantees that all pointers
    /// yielded by the iterator remain valid for the duration of the closure.
    ///
    /// # Examples
    ///
    /// ```
    /// # use infinity_pool::LocalPinnedPool;
    /// let mut pool = LocalPinnedPool::<u32>::new();
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
        F: FnOnce(LocalPinnedPoolIterator<'_, T>) -> R,
    {
        let guard = self.inner.borrow();
        let iter = LocalPinnedPoolIterator::new(&guard);
        f(iter)
    }
}

impl<T> Clone for LocalPinnedPool<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
            _phantom: PhantomData,
        }
    }
}

impl<T> Default for LocalPinnedPool<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> fmt::Debug for LocalPinnedPool<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalPinnedPool")
            .field("inner", &self.inner)
            .finish()
    }
}

/// Iterator over all objects in a local pinned pool.
///
/// The iterator only yields pointers to the objects, not references, because the pool
/// does not have the authority to create references to its contents as user code may
/// concurrently be holding a conflicting exclusive reference via `LocalPooledMut<T>`.
///
/// Therefore, obtaining actual references to pool contents via iteration is only possible
/// by using the pointer to create such references in unsafe code and relies on the caller
/// guaranteeing that no conflicting exclusive references exist.
#[derive(Debug)]
pub struct LocalPinnedPoolIterator<'p, T: 'static> {
    raw_iter: RawOpaquePoolIterator<'p>,
    _phantom: PhantomData<T>,
}

impl<'p, T> LocalPinnedPoolIterator<'p, T> {
    fn new(pool: &'p RawOpaquePool) -> Self {
        Self {
            raw_iter: pool.iter(),
            _phantom: PhantomData,
        }
    }
}

impl<T> Iterator for LocalPinnedPoolIterator<'_, T> {
    type Item = NonNull<T>;

    fn next(&mut self) -> Option<Self::Item> {
        self.raw_iter.next().map(NonNull::cast::<T>)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.raw_iter.size_hint()
    }
}

impl<T> DoubleEndedIterator for LocalPinnedPoolIterator<'_, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.raw_iter.next_back().map(NonNull::cast::<T>)
    }
}

impl<T> ExactSizeIterator for LocalPinnedPoolIterator<'_, T> {
    fn len(&self) -> usize {
        self.raw_iter.len()
    }
}

impl<T> FusedIterator for LocalPinnedPoolIterator<'_, T> {}

#[cfg(test)]
mod tests {
    use std::mem::MaybeUninit;

    use super::*;

    #[test]
    fn new_pool_is_empty() {
        let pool = LocalPinnedPool::<u32>::new();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity(), 0);
    }

    #[test]
    fn default_pool_is_empty() {
        let pool: LocalPinnedPool<String> = LocalPinnedPool::default();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity(), 0);
    }

    #[test]
    fn insert_and_length() {
        let mut pool = LocalPinnedPool::<u64>::new();

        let _h1 = pool.insert(10);
        assert_eq!(pool.len(), 1);
        assert!(!pool.is_empty());

        let _h2 = pool.insert(20);
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn capacity_grows_when_needed() {
        let mut pool = LocalPinnedPool::<u64>::new();

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
        let mut pool = LocalPinnedPool::<u8>::new();

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
        let mut pool = LocalPinnedPool::<u8>::new();

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
        let mut pool = LocalPinnedPool::<u8>::new();

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
        let mut pool = LocalPinnedPool::<u64>::new();

        let handle = pool.insert(12345_u64);

        assert_eq!(*handle, 12345);
    }

    #[test]
    fn multiple_handles_to_same_type() {
        let mut pool = LocalPinnedPool::<String>::new();

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
        let mut pool = LocalPinnedPool::<String>::new();

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
        let mut pool = LocalPinnedPool::<u64>::new();

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
        let mut p1 = LocalPinnedPool::<u32>::new();
        let mut p2 = p1.clone();

        let _h1 = p1.insert(100);
        assert_eq!(p2.len(), 1);

        let _h2 = p2.insert(200);
        assert_eq!(p1.len(), 2);

        // Both can iterate and see the same contents
        let mut values1: Vec<u32> = p1.with_iter(|iter| {
            iter.map(|ptr| {
                // SAFETY: Iterator yields valid NonNull<T> for items alive in pool
                unsafe { *ptr.as_ref() }
            })
            .collect()
        });
        let mut values2: Vec<u32> = p2.with_iter(|iter| {
            iter.map(|ptr| {
                // SAFETY: Iterator yields valid NonNull<T> for items alive in pool
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
            let mut pool = LocalPinnedPool::<String>::new();
            pool.insert("persist".to_string())
        }; // pool dropped here

        assert_eq!(&*handle, "persist");
        drop(handle); // drop removes from pool
    }

    #[test]
    fn lifecycle_pool_clone_keeps_inner_alive() {
        let mut pool = LocalPinnedPool::<String>::new();

        // Insert & clone
        let handle = pool.insert("data".to_string());
        let pool_clone = pool.clone();

        // Drop original
        drop(pool);

        assert_eq!(&*handle, "data");
        assert_eq!(pool_clone.len(), 1);

        drop(handle);
        assert_eq!(pool_clone.len(), 0);
    }

    #[test]
    fn pooled_mut_mutation_reflected() {
        let mut pool = LocalPinnedPool::<String>::new();

        let mut handle = pool.insert("hello".to_string());
        handle.push_str(" world");

        assert_eq!(&*handle, "hello world");
        assert_eq!(pool.len(), 1);

        pool.with_iter(|iter| {
            let vals: Vec<String> = iter
                .map(|p| {
                    // SAFETY: Iterator guarantees pointer validity for duration of closure.
                    unsafe { p.as_ref().clone() }
                })
                .collect();
            assert_eq!(vals, vec!["hello world".to_string()]);
        });
    }

    #[test]
    fn multiple_handles_and_drop() {
        let mut pool = LocalPinnedPool::<u32>::new();

        // Insert multiple objects
        let h1 = pool.insert(1);
        let h2 = pool.insert(2);
        let h3 = pool.insert(3);

        assert_eq!(pool.len(), 3);
        assert_eq!((*h1, *h2, *h3), (1, 2, 3));

        drop(h2);
        assert_eq!(pool.len(), 2);

        assert_eq!(*h1, 1);
        assert_eq!(*h3, 3);

        drop(h1);
        drop(h3);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn reserve_and_shrink_to_fit_shared() {
        let mut p1 = LocalPinnedPool::<u8>::new();
        let mut p2 = p1.clone();

        // Reserve via first handle
        p1.reserve(50);
        assert!(p1.capacity() >= 50);
        assert_eq!(p1.capacity(), p2.capacity());

        let h = p2.insert(99);
        assert_eq!(p1.len(), 1);
        drop(h);
        assert_eq!(p2.len(), 0);

        p1.shrink_to_fit();
        assert_eq!(p1.capacity(), p2.capacity());
    }

    #[test]
    fn with_iter_empty_pool() {
        let pool = LocalPinnedPool::<u32>::new();
        let count = pool.with_iter(|iter| iter.count());
        assert_eq!(count, 0);
    }

    #[test]
    fn with_iter_collect_values() {
        let mut pool = LocalPinnedPool::<u32>::new();

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
        let mut pool = LocalPinnedPool::<i32>::new();
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
        let pool = LocalPinnedPool::<u32>::new();
        pool.with_iter(|mut iter| {
            assert!(iter.next().is_none());
            assert!(iter.next().is_none());
            assert!(iter.next_back().is_none());
        });
    }

    #[test]
    fn iter_size_hint_and_exact_size() {
        let mut pool = LocalPinnedPool::<u32>::new();

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
    fn non_send_types() {
        // Custom non-Send type with raw pointer
        struct NonSendType(*const u8);
        // SAFETY: Only used in single-threaded local test environment, never shared across threads
        unsafe impl Sync for NonSendType {}

        // LocalPinnedPool should work with non-Send types since it's single-threaded
        use std::cell::RefCell;
        use std::rc::Rc;

        // Test with Rc (not Send)
        let mut rc_pool = LocalPinnedPool::<Rc<String>>::new();
        let rc_handle = rc_pool.insert(Rc::new("Non-Send data".to_string()));
        assert_eq!(rc_pool.len(), 1);
        assert_eq!(&**rc_handle, "Non-Send data");

        // Test with RefCell (not Send)
        let mut refcell_pool = LocalPinnedPool::<RefCell<i32>>::new();
        let refcell_handle = refcell_pool.insert(RefCell::new(42));
        assert_eq!(refcell_pool.len(), 1);
        assert_eq!(*refcell_handle.borrow(), 42);

        // Test with custom non-Send type
        let mut custom_pool = LocalPinnedPool::<NonSendType>::new();
        let raw_ptr = 0x1234 as *const u8;
        let non_send_handle = custom_pool.insert(NonSendType(raw_ptr));
        assert_eq!(custom_pool.len(), 1);
        assert_eq!(non_send_handle.0, raw_ptr);

        // Test iteration with non-Send types
        rc_pool.with_iter(|iter| {
            let values: Vec<String> = iter
                .map(|ptr| {
                    // SAFETY: Iterator yields valid NonNull<T> pointers for items alive in pool
                    unsafe { ptr.as_ref().as_ref().clone() }
                })
                .collect();
            assert_eq!(values, vec!["Non-Send data"]);
        });

        // Test nested non-Send types
        let mut nested_pool = LocalPinnedPool::<Rc<RefCell<Vec<i32>>>>::new();
        let nested_handle = nested_pool.insert(Rc::new(RefCell::new(vec![1, 2, 3])));
        assert_eq!(nested_pool.len(), 1);
        assert_eq!(*nested_handle.borrow(), vec![1, 2, 3]);
    }
}
