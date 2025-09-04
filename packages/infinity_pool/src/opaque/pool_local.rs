use std::alloc::Layout;
use std::cell::RefCell;
use std::iter::FusedIterator;
use std::mem::MaybeUninit;
use std::ptr::NonNull;
use std::rc::Rc;

use crate::{LocalPooledMut, RawOpaquePool, RawOpaquePoolIterator};

/// A pool of reference-counted objects with uniform memory layout.
///
/// Stores objects of any type that match a [`Layout`] defined at pool creation
/// time. All values in the pool remain pinned for their entire lifetime.
///
/// The pool automatically expands its capacity when needed.
///
/// # Lifetime management
///
/// The pool type itself acts as a handle - any clones of it are functionally equivalent,
/// similar to `Rc`.
///
/// When inserting an object into the pool, a handle to the object is returned.
/// The object is removed from the pool when the last remaining handle to the object
/// is dropped (`Rc`-like behavior).
///
/// # Thread safety
///
/// The pool is single-threaded.
#[derive(Debug)]
pub struct LocalOpaquePool {
    // The pool type itself is just a handle around the inner pool,
    // which is reference-counted and RefCell-guarded. The inner pool
    // will only ever be dropped once all items have been removed from
    // it and no more `LocalOpaquePool` instances exist that point to it.
    //
    // This also implies that `DropPolicy` has no meaning for this
    // pool configuration, as the pool can never be dropped if it has
    // contents (as dropping the handles of pooled objects will remove
    // them from the pool, while keeping the pool alive until then).
    inner: Rc<RefCell<RawOpaquePool>>,
}

impl LocalOpaquePool {
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

        Self {
            inner: Rc::new(RefCell::new(inner)),
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
        self.inner.borrow().object_layout()
    }

    /// The number of objects currently in the pool.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.borrow().len()
    }

    /// The total capacity of the pool.
    ///
    /// This is the maximum number of objects that the pool can contain without capacity extension.
    /// The pool will automatically extend its capacity if more than this many objects are inserted.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.inner.borrow().capacity()
    }

    /// Whether the pool contains zero objects.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.borrow().is_empty()
    }

    /// Ensures that the pool has capacity for at least `additional` more objects.
    ///
    /// # Panics
    ///
    /// Panics if the new capacity would exceed the size of virtual memory.
    pub fn reserve(&mut self, additional: usize) {
        self.inner.borrow_mut().reserve(additional);
    }

    /// Drops unused pool capacity to reduce memory usage.
    ///
    /// There is no guarantee that any unused capacity can be dropped. The exact outcome depends
    /// on the specific pool structure and which objects remain in the pool.
    pub fn shrink_to_fit(&mut self) {
        self.inner.borrow_mut().shrink_to_fit();
    }

    /// Inserts an object into the pool and returns a handle to it.
    ///
    /// # Panics
    ///
    /// Panics if the layout of `T` does not match the object layout of the pool.
    pub fn insert<T: Send>(&mut self, value: T) -> LocalPooledMut<T> {
        let inner = self.inner.borrow_mut().insert(value);

        LocalPooledMut::new(inner, Rc::clone(&self.inner))
    }

    /// Inserts an object into the pool and returns a handle to it.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the layout of `T` matches the pool's object layout.
    pub unsafe fn insert_unchecked<T: Send>(&mut self, value: T) -> LocalPooledMut<T> {
        // SAFETY: Forwarding safety guarantees from caller.
        let inner = unsafe { self.inner.borrow_mut().insert_unchecked(value) };

        LocalPooledMut::new(inner, Rc::clone(&self.inner))
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
    pub unsafe fn insert_with<T, F>(&mut self, f: F) -> LocalPooledMut<T>
    where
        T: Send,
        F: FnOnce(&mut MaybeUninit<T>),
    {
        // SAFETY: Forwarding safety guarantees from caller.
        let inner = unsafe { self.inner.borrow_mut().insert_with(f) };

        LocalPooledMut::new(inner, Rc::clone(&self.inner))
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
    pub unsafe fn insert_with_unchecked<T, F>(&mut self, f: F) -> LocalPooledMut<T>
    where
        T: Send,
        F: FnOnce(&mut MaybeUninit<T>),
    {
        // SAFETY: Forwarding safety guarantees from caller.
        let inner = unsafe { self.inner.borrow_mut().insert_with_unchecked(f) };

        LocalPooledMut::new(inner, Rc::clone(&self.inner))
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
    /// # use infinity_pool::LocalOpaquePool;
    /// let mut pool = LocalOpaquePool::with_layout_of::<u32>();
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
        F: FnOnce(LocalOpaquePoolIterator<'_>) -> R,
    {
        let guard = self.inner.borrow();
        let iter = LocalOpaquePoolIterator::new(&guard);
        f(iter)
    }
}

impl Clone for LocalOpaquePool {
    fn clone(&self) -> Self {
        Self {
            inner: Rc::clone(&self.inner),
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
pub struct LocalOpaquePoolIterator<'p> {
    raw_iter: RawOpaquePoolIterator<'p>,
}

impl<'p> LocalOpaquePoolIterator<'p> {
    fn new(pool: &'p RawOpaquePool) -> Self {
        Self {
            raw_iter: pool.iter(),
        }
    }
}

impl Iterator for LocalOpaquePoolIterator<'_> {
    type Item = NonNull<()>;

    fn next(&mut self) -> Option<Self::Item> {
        self.raw_iter.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.raw_iter.size_hint()
    }
}

impl DoubleEndedIterator for LocalOpaquePoolIterator<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.raw_iter.next_back()
    }
}

impl ExactSizeIterator for LocalOpaquePoolIterator<'_> {
    fn len(&self) -> usize {
        self.raw_iter.len()
    }
}

impl FusedIterator for LocalOpaquePoolIterator<'_> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iter_empty_pool() {
        let pool = LocalOpaquePool::with_layout_of::<u32>();

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
        let mut pool = LocalOpaquePool::with_layout_of::<u32>();

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
        let mut pool = LocalOpaquePool::with_layout_of::<u32>();

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
        let mut pool = LocalOpaquePool::with_layout_of::<u32>();

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
        let mut pool = LocalOpaquePool::with_layout_of::<u32>();

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
        let mut pool = LocalOpaquePool::with_layout_of::<u32>();

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
    fn arc_like_clone_behavior() {
        let mut pool1 = LocalOpaquePool::with_layout_of::<u32>();

        // Clone the pool handle
        let mut pool2 = pool1.clone();

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
        let mut pool = LocalOpaquePool::with_layout_of::<String>();

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
            let mut pool = LocalOpaquePool::with_layout_of::<String>();
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
    fn pooled_mut_integration() {
        let mut pool = LocalOpaquePool::with_layout_of::<String>();

        // Test that PooledMut works correctly with LocalOpaquePool
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
        let mut pool = LocalOpaquePool::with_layout_of::<u32>();

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
        let mut pool = LocalOpaquePool::with_layout_of::<String>();

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
        let mut pool1 = LocalOpaquePool::with_layout_of::<u32>();

        // Insert some initial data
        let _handle1 = pool1.insert(100_u32);
        let _handle2 = pool1.insert(200_u32);

        // Clone the pool
        let mut pool2 = pool1.clone();

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
        let mut pool = LocalOpaquePool::with_layout_of::<String>();

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
}
