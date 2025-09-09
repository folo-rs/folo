use std::alloc::Layout;
use std::iter::FusedIterator;
use std::mem::MaybeUninit;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use crate::opaque::pool_raw::RawOpaquePoolIterator;
use crate::{ERR_POISONED_LOCK, PooledMut, RawOpaquePool, RawOpaquePoolSend};

/// A thread-safe pool of reference-counted objects with uniform memory layout.
///
/// Stores objects of any `Send` type that match a [`Layout`] defined at pool creation
/// time. All values in the pool remain pinned for their entire lifetime.
///
/// The pool automatically expands its capacity when needed.
#[doc = include_str!("../../doc/snippets/managed_pool_lifetimes.md")]
#[doc = include_str!("../../doc/snippets/managed_pool_is_thread_safe.md")]
///
/// # Example
///
/// ```rust
/// use infinity_pool::OpaquePool;
///
/// fn work_with_displayable<T: std::fmt::Display + Send + 'static>(value: T) {
///     let mut pool = OpaquePool::with_layout_of::<T>();
///
///     // Insert an object into the pool
///     let handle = pool.insert(value);
///
///     // Access the object through the handle
///     println!("Stored: {}", &*handle);
///
///     // The object is automatically removed when the handle is dropped
/// }
///
/// work_with_displayable("Hello, world!");
/// work_with_displayable(42);
/// ```
///
/// # Pool clones are functionally equivalent
///
/// ```rust
/// use infinity_pool::OpaquePool;
///
/// let mut pool1 = OpaquePool::with_layout_of::<i32>();
/// let pool2 = pool1.clone();
///
/// assert_eq!(pool1.len(), pool2.len());
/// let _handle = pool1.insert(42_i32);
/// assert_eq!(pool1.len(), pool2.len());
/// ```
#[derive(Debug)]
pub struct OpaquePool {
    // We require 'static from any inserted values because the pool
    // does not enforce any Rust lifetime semantics, only reference counts.
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

    #[doc = include_str!("../../doc/snippets/opaque_pool_layout.md")]
    #[must_use]
    #[inline]
    pub fn object_layout(&self) -> Layout {
        self.inner.lock().expect(ERR_POISONED_LOCK).object_layout()
    }

    #[doc = include_str!("../../doc/snippets/pool_len.md")]
    #[must_use]
    #[inline]
    pub fn len(&self) -> usize {
        self.inner.lock().expect(ERR_POISONED_LOCK).len()
    }

    #[doc = include_str!("../../doc/snippets/pool_capacity.md")]
    #[must_use]
    #[inline]
    pub fn capacity(&self) -> usize {
        self.inner.lock().expect(ERR_POISONED_LOCK).capacity()
    }

    #[doc = include_str!("../../doc/snippets/pool_is_empty.md")]
    #[must_use]
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.inner.lock().expect(ERR_POISONED_LOCK).is_empty()
    }

    #[doc = include_str!("../../doc/snippets/pool_reserve.md")]
    #[inline]
    pub fn reserve(&self, additional: usize) {
        self.inner
            .lock()
            .expect(ERR_POISONED_LOCK)
            .reserve(additional);
    }

    #[doc = include_str!("../../doc/snippets/pool_shrink_to_fit.md")]
    #[inline]
    pub fn shrink_to_fit(&self) {
        self.inner.lock().expect(ERR_POISONED_LOCK).shrink_to_fit();
    }

    #[doc = include_str!("../../doc/snippets/pool_insert.md")]
    ///
    ///  # Panics
    #[doc = include_str!("../../doc/snippets/panic_on_pool_t_layout_mismatch.md")]
    ///
    /// # Example
    ///
    /// ```rust
    /// use infinity_pool::OpaquePool;
    ///
    /// let mut pool = OpaquePool::with_layout_of::<String>();
    ///
    /// // Insert an object into the pool
    /// let mut handle = pool.insert("Hello".to_string());
    ///
    /// // Mutate the object via the unique handle
    /// handle.push_str(", Opaque World!");
    /// assert_eq!(&*handle, "Hello, Opaque World!");
    ///
    /// // Transform the unique handle into a shared handle
    /// let shared_handle = handle.into_shared();
    ///
    /// // After transformation, you can only immutably dereference the object
    /// assert_eq!(&*shared_handle, "Hello, Opaque World!");
    /// // shared_handle.push_str("!"); // This would not compile
    ///
    /// // The object is removed when the handle is dropped
    /// drop(shared_handle); // Explicitly drop to remove from pool
    /// assert_eq!(pool.len(), 0);
    /// ```
    #[inline]
    #[must_use]
    pub fn insert<T: Send + 'static>(&self, value: T) -> PooledMut<T> {
        let inner = self.inner.lock().expect(ERR_POISONED_LOCK).insert(value);

        PooledMut::new(inner, Arc::clone(&self.inner))
    }

    #[doc = include_str!("../../doc/snippets/pool_insert.md")]
    /// # Safety
    #[doc = include_str!("../../doc/snippets/safety_pool_t_layout_must_match.md")]
    #[inline]
    #[must_use]
    pub unsafe fn insert_unchecked<T: Send + 'static>(&self, value: T) -> PooledMut<T> {
        // SAFETY: Forwarding safety guarantees from caller.
        let inner = unsafe {
            self.inner
                .lock()
                .expect(ERR_POISONED_LOCK)
                .insert_unchecked(value)
        };

        PooledMut::new(inner, Arc::clone(&self.inner))
    }

    #[doc = include_str!("../../doc/snippets/pool_insert_with.md")]
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::mem::MaybeUninit;
    ///
    /// use infinity_pool::OpaquePool;
    ///
    /// struct DataBuffer {
    ///     id: u32,
    ///     data: MaybeUninit<[u8; 1024]>, // Large buffer to skip initializing
    /// }
    ///
    /// let mut pool = OpaquePool::with_layout_of::<DataBuffer>();
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
    /// # Panics
    #[doc = include_str!("../../doc/snippets/panic_on_pool_t_layout_mismatch.md")]
    ///
    /// # Safety
    #[doc = include_str!("../../doc/snippets/safety_closure_must_initialize_object.md")]
    #[inline]
    #[must_use]
    pub unsafe fn insert_with<T, F>(&self, f: F) -> PooledMut<T>
    where
        T: Send + 'static,
        F: FnOnce(&mut MaybeUninit<T>),
    {
        // SAFETY: Forwarding safety guarantees from caller.
        let inner = unsafe { self.inner.lock().expect(ERR_POISONED_LOCK).insert_with(f) };

        PooledMut::new(inner, Arc::clone(&self.inner))
    }

    #[doc = include_str!("../../doc/snippets/pool_insert_with.md")]
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::mem::MaybeUninit;
    ///
    /// use infinity_pool::OpaquePool;
    ///
    /// struct DataBuffer {
    ///     id: u32,
    ///     data: MaybeUninit<[u8; 1024]>, // Large buffer to skip initializing
    /// }
    ///
    /// let mut pool = OpaquePool::with_layout_of::<DataBuffer>();
    ///
    /// // Initialize only the id, leaving data uninitialized for performance
    /// let handle = unsafe {
    ///     pool.insert_with_unchecked(|uninit: &mut MaybeUninit<DataBuffer>| {
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
    #[doc = include_str!("../../doc/snippets/safety_pool_t_layout_must_match.md")]
    #[doc = include_str!("../../doc/snippets/safety_closure_must_initialize_object.md")]
    #[inline]
    #[must_use]
    pub unsafe fn insert_with_unchecked<T, F>(&self, f: F) -> PooledMut<T>
    where
        T: Send + 'static,
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
    ///     iter.map(|ptr| unsafe { *ptr.cast::<u32>().as_ref() })
    ///         .collect()
    /// });
    /// ```
    pub fn with_iter<F, R>(&self, f: F) -> R
    where
        F: FnOnce(OpaquePoolIterator<'_>) -> R,
    {
        let guard = self.inner.lock().expect(ERR_POISONED_LOCK);
        let iter = OpaquePoolIterator::new(&guard);
        f(iter)
    }
}

impl Clone for OpaquePool {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
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
    fn new_pool_with_layout_of_is_empty() {
        let pool = OpaquePool::with_layout_of::<u64>();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity(), 0);
        assert_eq!(pool.object_layout(), Layout::new::<u64>());
    }

    #[test]
    fn new_pool_with_layout_is_empty() {
        let layout = Layout::new::<i64>();
        let pool = OpaquePool::with_layout(layout);

        assert_eq!(pool.object_layout(), layout);
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
        assert_eq!(pool.capacity(), 0);
    }

    #[test]
    fn insert_and_length() {
        let pool = OpaquePool::with_layout_of::<u32>();

        let _handle1 = pool.insert(42_u32);
        assert_eq!(pool.len(), 1);
        assert!(!pool.is_empty());

        let _handle2 = pool.insert(100_u32);
        assert_eq!(pool.len(), 2);
    }

    #[test]
    fn capacity_grows_when_needed() {
        let pool = OpaquePool::with_layout_of::<u64>();

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
        let pool = OpaquePool::with_layout_of::<u8>();

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
        let pool = OpaquePool::with_layout_of::<u64>();

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
    fn shrink_to_fit_removes_unused_capacity() {
        let pool = OpaquePool::with_layout_of::<u8>();

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
        let pool = OpaquePool::with_layout_of::<u8>();

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
        let pool = OpaquePool::with_layout_of::<u64>();

        let handle = pool.insert(12345_u64);

        assert_eq!(*handle, 12345);
    }

    #[test]
    fn multiple_handles_to_same_type() {
        let pool = OpaquePool::with_layout_of::<String>();

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
        let pool = OpaquePool::with_layout_of::<String>();

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
    fn iter_empty_pool() {
        let pool = OpaquePool::with_layout_of::<u32>();

        pool.with_iter(|mut iter| {
            assert_eq!(iter.size_hint(), (0, Some(0)));
            assert_eq!(iter.len(), 0);

            assert_eq!(iter.next(), None);
            assert_eq!(iter.size_hint(), (0, Some(0)));
            assert_eq!(iter.len(), 0);
        });
    }

    #[test]
    fn iter_single_item() {
        let pool = OpaquePool::with_layout_of::<u32>();

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
    }

    #[test]
    fn iter_multiple_items() {
        let pool = OpaquePool::with_layout_of::<u32>();

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
    }

    #[test]
    fn iter_double_ended_basic() {
        let pool = OpaquePool::with_layout_of::<u32>();

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
    }

    #[test]
    fn with_iter_scoped_access() {
        let pool = OpaquePool::with_layout_of::<u32>();

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
    }

    #[test]
    fn with_iter_holds_lock() {
        let pool = OpaquePool::with_layout_of::<u32>();

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

    #[test]
    fn iter_size_hint_and_exact_size() {
        let pool = OpaquePool::with_layout_of::<u32>();

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
    fn clone_behavior() {
        let pool1 = OpaquePool::with_layout_of::<u32>();

        // Clone the pool handle
        let pool2 = pool1.clone();

        // Both should have the same object layout
        assert_eq!(pool1.object_layout(), pool2.object_layout());

        // Insert via first handle
        let _handle1 = pool1.insert(100_u32);

        // Second handle should see the same pool state
        assert_eq!(pool2.len(), 1);
        assert!(!pool2.is_empty());

        // Insert via second handle
        let _handle2 = pool2.insert(200_u32);

        // First handle should see the updated state
        assert_eq!(pool1.len(), 2);

        // Both handles should see the objects via iteration
        pool1.with_iter(|iter| {
            let values: Vec<u32> = iter
                // SAFETY: We know these point to u32s we inserted
                .map(|ptr| unsafe { *ptr.cast::<u32>().as_ref() })
                .collect();
            assert_eq!(values, vec![100, 200]);
        });

        pool2.with_iter(|iter| {
            let values: Vec<u32> = iter
                // SAFETY: We know these point to u32s we inserted
                .map(|ptr| unsafe { *ptr.cast::<u32>().as_ref() })
                .collect();
            assert_eq!(values, vec![100, 200]);
        });
    }

    #[test]
    fn lifecycle_management_pool_keeps_inner_alive() {
        let pool = OpaquePool::with_layout_of::<String>();

        // Insert an object and get a handle
        let handle = pool.insert("test data".to_string());

        // Clone the pool before dropping the original
        let pool_clone = pool.clone();

        // Drop the original pool
        drop(pool);

        // The handle should still be valid because pool_clone keeps the inner pool alive
        assert_eq!(&*handle, "test data");

        // We should still be able to access pool operations through the clone
        assert_eq!(pool_clone.len(), 1);
        assert!(!pool_clone.is_empty());

        // Dropping the handle should remove the object
        drop(handle);

        // Pool clone should reflect the removal
        assert_eq!(pool_clone.len(), 0);
        assert!(pool_clone.is_empty());
    }

    #[test]
    fn lifecycle_management_handles_keep_pool_alive() {
        let handle = {
            let pool = OpaquePool::with_layout_of::<String>();
            // Pool goes out of scope here, but handle should keep inner pool alive
            pool.insert("persistent data".to_string())
        };

        // Handle should still be valid and accessible
        assert_eq!(&*handle, "persistent data");

        // Even though the original pool is dropped, the object remains accessible
        // This tests that PooledMut holds a reference to the inner pool
        drop(handle);
        // Object is cleaned up when handle is dropped
    }

    #[test]
    fn concurrent_access_basic() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let pool = Arc::new(Mutex::new(OpaquePool::with_layout_of::<u32>()));
        let barrier = Arc::new(Barrier::new(3));

        let mut handles = vec![];

        // Spawn threads that insert concurrently
        for i in 0..3 {
            let pool_clone = Arc::clone(&pool);
            let barrier_clone = Arc::clone(&barrier);

            let handle = thread::spawn(move || {
                barrier_clone.wait();

                let value = (i + 1) * 100;
                let handle = {
                    let pool_guard = pool_clone.lock().unwrap();
                    pool_guard.insert(value)
                };

                // Return the handle and the value we inserted
                (handle, value)
            });

            handles.push(handle);
        }

        // Collect results and keep the pooled handles alive
        let mut pooled_handles = vec![];
        for handle in handles {
            let (pooled_handle, value) = handle.join().unwrap();
            assert_eq!(*pooled_handle, value);
            pooled_handles.push(pooled_handle);
        }

        // Verify final pool state
        {
            let pool_guard = pool.lock().unwrap();
            assert_eq!(pool_guard.len(), 3);

            let mut values: Vec<u32> = pool_guard.with_iter(|iter| {
                iter
                    // SAFETY: We know these point to u32s we inserted
                    .map(|ptr| unsafe { *ptr.cast::<u32>().as_ref() })
                    .collect()
            });

            values.sort_unstable();
            assert_eq!(values, vec![100, 200, 300]);
        }

        // Clean up by dropping the handles
        drop(pooled_handles);
    }

    #[test]
    fn pooled_mut_integration() {
        let pool = OpaquePool::with_layout_of::<String>();

        // Test that PooledMut works correctly with OpaquePool
        let mut handle = pool.insert("initial".to_string());

        // Test deref access
        assert_eq!(&*handle, "initial");

        // Test mutable access
        handle.push_str(" value");
        assert_eq!(&*handle, "initial value");

        // Test that the pool sees the mutation
        assert_eq!(pool.len(), 1);

        // Test that iteration sees the mutated value
        pool.with_iter(|iter| {
            let values: Vec<String> = iter
                // SAFETY: We know these point to Strings we inserted
                .map(|ptr| unsafe { ptr.cast::<String>().as_ref().clone() })
                .collect();
            assert_eq!(values, vec!["initial value"]);
        });

        // Test that dropping the handle removes from pool
        drop(handle);
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn multiple_handles_to_different_objects() {
        let pool = OpaquePool::with_layout_of::<u32>();

        // Insert multiple objects
        let handle1 = pool.insert(100_u32);
        let handle2 = pool.insert(200_u32);
        let handle3 = pool.insert(300_u32);

        assert_eq!(pool.len(), 3);

        // All handles should be independently accessible
        assert_eq!(*handle1, 100);
        assert_eq!(*handle2, 200);
        assert_eq!(*handle3, 300);

        // Drop one handle
        drop(handle2);
        assert_eq!(pool.len(), 2);

        // Remaining handles should still work
        assert_eq!(*handle1, 100);
        assert_eq!(*handle3, 300);

        // Pool should reflect the removal
        pool.with_iter(|iter| {
            let mut values: Vec<u32> = iter
                // SAFETY: We know these point to u32s we inserted
                .map(|ptr| unsafe { *ptr.cast::<u32>().as_ref() })
                .collect();
            values.sort_unstable();
            assert_eq!(values, vec![100, 300]);
        });

        drop(handle1);
        drop(handle3);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn insert_methods_with_lifecycle() {
        let pool = OpaquePool::with_layout_of::<String>();

        // Test regular insert
        let handle1 = pool.insert("regular".to_string());
        assert_eq!(pool.len(), 1);

        // Test unsafe insert_unchecked
        // SAFETY: String layout matches the pool's object layout
        let handle2 = unsafe { pool.insert_unchecked("unchecked".to_string()) };
        assert_eq!(pool.len(), 2);

        // Test insert_with
        // SAFETY: We properly initialize the string value
        let handle3 = unsafe {
            pool.insert_with(|uninit| {
                uninit.write("with_closure".to_string());
            })
        };
        assert_eq!(pool.len(), 3);

        // Test insert_with_unchecked
        // SAFETY: String layout matches the pool and we properly initialize the value
        let handle4 = unsafe {
            pool.insert_with_unchecked(|uninit| {
                uninit.write("with_closure_unchecked".to_string());
            })
        };
        assert_eq!(pool.len(), 4);

        // Verify all values
        assert_eq!(&*handle1, "regular");
        assert_eq!(&*handle2, "unchecked");
        assert_eq!(&*handle3, "with_closure");
        assert_eq!(&*handle4, "with_closure_unchecked");

        // Test cleanup
        drop(handle1);
        assert_eq!(pool.len(), 3);

        drop(handle2);
        drop(handle3);
        drop(handle4);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn pool_operations_with_arc_semantics() {
        let pool1 = OpaquePool::with_layout_of::<u32>();

        // Insert some initial data
        let _handle1 = pool1.insert(100_u32);
        let _handle2 = pool1.insert(200_u32);

        // Clone the pool
        let pool2 = pool1.clone();

        // Test capacity operations on both handles
        assert_eq!(pool1.capacity(), pool2.capacity());

        let initial_capacity = pool1.capacity();
        pool1.reserve(10);

        // Both should see the capacity change (note: reserve is a minimum, actual capacity may be higher)
        assert!(pool1.capacity() >= initial_capacity);
        assert_eq!(pool1.capacity(), pool2.capacity());

        // Test shrink_to_fit
        pool2.shrink_to_fit();
        assert_eq!(pool1.capacity(), pool2.capacity());

        // Both should see the same length and state
        assert_eq!(pool1.len(), pool2.len());
        assert_eq!(pool1.is_empty(), pool2.is_empty());
        assert_eq!(pool1.object_layout(), pool2.object_layout());
    }

    #[test]
    fn into_inner_removes_and_returns_value() {
        let pool = OpaquePool::with_layout_of::<String>();

        // Insert an item into the pool
        let mut handle = pool.insert("initial value".to_string());
        assert_eq!(pool.len(), 1);
        assert_eq!(&*handle, "initial value");

        // Modify the value while it's in the pool
        handle.push_str(" - modified");
        assert_eq!(&*handle, "initial value - modified");
        assert_eq!(pool.len(), 1);

        // Extract the value from the pool using into_inner()
        let extracted_value = handle.into_inner();

        // Verify the extracted value is correct
        assert_eq!(extracted_value, "initial value - modified");

        // Verify the pool is now empty (item was removed)
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());

        // The caller now owns the value and can continue using it
        let final_value = extracted_value + " - after extraction";
        assert_eq!(final_value, "initial value - modified - after extraction");
    }

    #[test]
    fn pool_operations_work_with_shared_references() {
        // Test that all insertion methods work with &self (shared references)
        let pool = OpaquePool::with_layout_of::<String>();

        // Test basic insert
        let handle1 = pool.insert("hello".to_string());
        assert_eq!(pool.len(), 1);
        assert_eq!(&*handle1, "hello");

        // Test insert_with
        // SAFETY: We properly initialize the value in the closure.
        let handle2 = unsafe {
            pool.insert_with(|uninit| {
                uninit.write("world".to_string());
            })
        };
        assert_eq!(pool.len(), 2);
        assert_eq!(&*handle2, "world");

        // Test insert_unchecked
        // SAFETY: String layout matches the expected layout for this pool.
        let handle3 = unsafe { pool.insert_unchecked("test".to_string()) };
        assert_eq!(pool.len(), 3);
        assert_eq!(&*handle3, "test");

        // Test insert_with_unchecked
        // SAFETY: We properly initialize the value in the closure.
        let handle4 = unsafe {
            pool.insert_with_unchecked(|uninit| {
                uninit.write("unchecked".to_string());
            })
        };
        assert_eq!(pool.len(), 4);
        assert_eq!(&*handle4, "unchecked");

        // Test reserve and shrink_to_fit work with shared references
        pool.reserve(10);
        pool.shrink_to_fit();

        // Clean up
        drop(handle1);
        drop(handle2);
        drop(handle3);
        drop(handle4);
        assert_eq!(pool.len(), 0);
    }
}
