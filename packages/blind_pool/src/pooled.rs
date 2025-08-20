use std::fmt;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::Arc;

use crate::{BlindPool, RawPooled};

/// A reference to a value stored in a [`BlindPool`].
///
/// This type provides automatic lifetime management for values in the pool.
/// When the last [`Pooled`] instance for a value is dropped, the value
/// is automatically removed from the pool.
///
/// Multiple [`Pooled`] instances can reference the same value through
/// cloning, implementing reference counting semantics.
///
/// # Thread Safety
///
/// [`Pooled<T>`] implements thread safety traits conditionally based on the stored type `T`:
///
/// - **Send**: [`Pooled<T>`] is [`Send`] if and only if `T` is [`Sync`]. This allows moving
///   pooled references between threads when the referenced type supports concurrent access.
///
/// - **Sync**: [`Pooled<T>`] is [`Sync`] if and only if `T` is [`Sync`]. This allows sharing
///   the same [`Pooled<T>`] instance between multiple threads when the referenced type supports
///   concurrent access.
///
/// # Trait Objects
///
/// You can convert to trait objects using the standard dereferencing approach:
///
/// ```rust
/// use blind_pool::BlindPool;
///
/// trait MyTrait {
///     fn do_something(&self);
/// }
///
/// struct MyStruct(u32);
/// impl MyTrait for MyStruct {
///     fn do_something(&self) {
///         println!("Doing something with value: {}", self.0);
///     }
/// }
///
/// let pool = BlindPool::new();
/// let handle = pool.insert(MyStruct(42));
///
/// // Convert to trait object using standard dereferencing
/// let trait_ref: &dyn MyTrait = &*handle;
/// trait_ref.do_something();
/// ```
///
/// # Example
///
/// ```rust
/// use blind_pool::BlindPool;
///
/// let pool = BlindPool::new();
/// let value_handle = pool.insert("Test".to_string());
///
/// // Access the value through dereferencing.
/// assert_eq!(*value_handle, "Test".to_string());
///
/// // Clone to create additional references.
/// let cloned_handle = value_handle.clone();
/// assert_eq!(*cloned_handle, "Test".to_string());
/// ```
pub struct Pooled<T: ?Sized> {
    /// The reference-counted inner data containing the actual pooled item and pool handle.
    inner: Arc<PooledInner<T>>,
}

/// Internal data structure that manages the lifetime of a pooled item.
///
/// This is always type-erased to `()` and shared among all typed views of the same item.
/// It ensures that the item is removed from the pool exactly once when all references are dropped.
struct PooledRef {
    /// The type-erased handle to the actual item in the pool.
    pooled: RawPooled<()>,

    /// A handle to the pool that keeps it alive as long as this item exists.
    pool: BlindPool,
}

/// Internal data structure that contains the typed access to the pooled item.
struct PooledInner<T: ?Sized> {
    /// The typed handle to the actual item in the pool.
    pooled: RawPooled<T>,

    /// A shared reference to the lifetime manager.
    lifetime: Arc<PooledRef>,
}

impl<T: ?Sized> PooledInner<T> {
    /// Creates a new `PooledInner` with the given components.
    ///
    /// This method is intended for internal use when reconstructing pooled values
    /// after type casting operations via the [`define_pooled_dyn_cast!`] macro.
    #[must_use]
    fn new(pooled: RawPooled<T>, pool: BlindPool) -> Self {
        let lifetime = Arc::new(PooledRef {
            pooled: pooled.erase(),
            pool,
        });
        Self { pooled, lifetime }
    }

    /// Creates a new `PooledInner` sharing the lifetime with an existing one.
    ///
    /// This is used for type casting operations where we want to create a new typed view
    /// while sharing the same lifetime management.
    #[must_use]
    fn with_shared_lifetime(pooled: RawPooled<T>, lifetime: Arc<PooledRef>) -> Self {
        Self { pooled, lifetime }
    }
}

impl<T: ?Sized> Pooled<T> {
    /// Creates a new pooled value.
    ///
    /// This method is intended for internal use by [`BlindPool`] and for
    /// reconstructing pooled values after type casting via the [`define_pooled_dyn_cast!`] macro.
    #[must_use]
    pub(crate) fn new(pooled: RawPooled<T>, pool: BlindPool) -> Self {
        let inner = PooledInner::new(pooled, pool);
        Self {
            inner: Arc::new(inner),
        }
    }

    /// Erases the type information from this [`Pooled<T>`] handle,
    /// returning a [`Pooled<()>`].
    ///
    /// This is useful when you want to store handles of different types in the same collection
    /// or pass them to code that doesn't need to know the specific type.
    ///
    /// The returned handle shares the same underlying reference count as the original handle.
    /// Multiple handles (both typed and type-erased) can coexist for the same pooled item,
    /// and the item will only be removed from the pool when all handles are dropped.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let pool = BlindPool::new();
    /// let value_handle = pool.insert("Test".to_string());
    /// let cloned_handle = value_handle.clone();
    ///
    /// // Erase type information while keeping the original handle via clone.
    /// let erased = value_handle.erase();
    ///
    /// // Both handles are valid and refer to the same item.
    /// assert_eq!(*cloned_handle, "Test".to_string());
    ///
    /// // The erased handle shares the same reference count.
    /// drop(erased);
    /// assert_eq!(*cloned_handle, "Test".to_string()); // Still accessible via typed handle.
    /// ```
    #[must_use]
    #[inline]
    pub fn erase(self) -> Pooled<()> {
        // Create a new erased handle sharing the same lifetime manager
        let erased_pooled = self.inner.pooled.erase();
        let erased_inner =
            PooledInner::with_shared_lifetime(erased_pooled, Arc::clone(&self.inner.lifetime));

        Pooled {
            inner: Arc::new(erased_inner),
        }
    }

    /// Returns a pointer to the stored value.
    ///
    /// This provides direct access to the underlying pointer while maintaining the safety
    /// guarantees of the pooled reference. The pointer remains valid as long as any
    /// [`Pooled<T>`] handle exists for the same value.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let pool = BlindPool::new();
    /// let value_handle = pool.insert("Test".to_string());
    ///
    /// // Get the pointer to the stored value.
    /// let ptr = value_handle.ptr();
    ///
    /// // SAFETY: The pointer is valid as long as value_handle exists.
    /// let value = unsafe { ptr.as_ref() };
    /// assert_eq!(value, "Test");
    /// ```
    #[must_use]
    #[inline]
    pub fn ptr(&self) -> std::ptr::NonNull<T> {
        self.inner.pooled.ptr()
    }

    /// Returns a pinned reference to the value stored in the pool.
    ///
    /// Since values in the pool are always pinned (they never move once inserted),
    /// this method provides safe access to `Pin<&T>` without requiring unsafe code.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::pin::Pin;
    ///
    /// use blind_pool::BlindPool;
    ///
    /// let pool = BlindPool::new();
    /// let handle = pool.insert("hello".to_string());
    ///
    /// let pinned: Pin<&String> = handle.as_pin();
    /// assert_eq!(pinned.len(), 5);
    /// ```
    #[must_use]
    #[inline]
    pub fn as_pin(&self) -> Pin<&T> {
        // SAFETY: Values in the pool are always pinned - they never move once inserted.
        // The pool ensures stable addresses for the lifetime of the pooled object.
        unsafe { Pin::new_unchecked(&**self) }
    }

    /// Casts this [`Pooled<T>`] to a trait object type.
    ///
    /// This method converts a pooled value from a concrete type to a trait object
    /// while preserving the reference counting and pool management semantics.
    ///
    /// The returned handle shares the same underlying reference count as the original handle.
    /// Multiple handles (both concrete type and trait object) can coexist for the same pooled item,
    /// and the item will only be removed from the pool when all handles are dropped.
    ///
    /// This method is only intended for use by the [`define_pooled_dyn_cast!`] macro.
    #[must_use]
    #[doc(hidden)]
    #[inline]
    pub fn __private_cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> Pooled<U>
    where
        F: FnOnce(&T) -> &U,
    {
        // Cast the RawPooled to the trait object using the provided function
        // SAFETY: The lifetime management logic of this pool guarantees that the target item is
        // still alive in the pool for as long as any handle exists, which it clearly does.
        // We only ever hand out shared references to the item, so no conflicting `&mut`
        // exclusive references can exist.
        let cast_pooled = unsafe { self.inner.pooled.__private_cast_dyn_with_fn(cast_fn) };
        let cast_inner =
            PooledInner::with_shared_lifetime(cast_pooled, Arc::clone(&self.inner.lifetime));

        Pooled {
            inner: Arc::new(cast_inner),
        }
    }
}

impl<T: ?Sized> Clone for Pooled<T> {
    /// Creates another handle to the same pooled value.
    ///
    /// This increases the reference count for the underlying value. The value will only be
    /// removed from the pool when all cloned handles are dropped.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let pool = BlindPool::new();
    /// let value_handle = pool.insert("Test".to_string());
    ///
    /// let cloned_handle = value_handle.clone();
    ///
    /// // Both handles refer to the same value.
    /// assert_eq!(*value_handle, *cloned_handle);
    ///
    /// // Value remains in pool until all handles are dropped.
    /// drop(value_handle);
    /// assert_eq!(*cloned_handle, "Test".to_string()); // Still accessible.
    /// ```
    #[inline]
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T: ?Sized> Deref for Pooled<T> {
    type Target = T;

    /// Provides direct access to the value stored in the pool.
    ///
    /// This allows the handle to be used as if it were a reference to the stored value.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let pool = BlindPool::new();
    /// let string_handle = pool.insert("hello".to_string());
    ///
    /// // Access string methods directly.
    /// assert_eq!(string_handle.len(), 5);
    /// assert!(string_handle.starts_with("he"));
    /// ```
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: The pooled handle is valid and contains initialized memory of type T.
        // The Arc reference count ensures the underlying pool data remains alive during access.
        // We only ever hand out shared references, so no exclusive reference can exist.
        unsafe { self.inner.pooled.ptr().as_ref() }
    }
}

impl Drop for PooledRef {
    /// Automatically removes the item from the pool when the last reference is dropped.
    ///
    /// This ensures that resources are properly cleaned up without requiring manual intervention.
    #[inline]
    fn drop(&mut self) {
        // We are guaranteed to be the only one executing this drop because Arc ensures
        // that Drop on the PooledRef is only called once when the last reference is released.
        self.pool.remove(&self.pooled);
    }
}

// SAFETY: Pooled<T> can be Send if T is Sync, because multiple threads could
// access the same referenced data when the Pooled<T> is moved between threads.
// The Arc<PooledInner<T>> is Send when T is Sync, and the mutex in BlindPool provides thread safety.
unsafe impl<T: Sync> Send for Pooled<T> {}

// SAFETY: Pooled<T> can be Sync if T is Sync, because multiple threads can safely
// access the same Pooled<T> instance if T is Sync. The deref operation is safe
// for concurrent access when T is Sync, and other operations don't require exclusive access.
unsafe impl<T: Sync> Sync for Pooled<T> {}

impl<T: ?Sized> fmt::Debug for Pooled<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pooled")
            .field("type_name", &std::any::type_name::<T>())
            .field("ptr", &self.inner.pooled.ptr())
            .finish()
    }
}

impl<T: ?Sized> fmt::Debug for PooledInner<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PooledInner")
            .field("type_name", &std::any::type_name::<T>())
            .field("ptr", &self.pooled.ptr())
            .field("lifetime", &self.lifetime)
            .finish()
    }
}

impl fmt::Debug for PooledRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PooledRef")
            .field("pooled", &self.pooled)
            .field("pool", &self.pool)
            .finish()
    }
}

// SAFETY: PooledRef can be Send because both RawPooled<()> and BlindPool are Send.
unsafe impl Send for PooledRef {}

// SAFETY: PooledRef can be Sync because both RawPooled<()> and BlindPool are Sync.
unsafe impl Sync for PooledRef {}

// SAFETY: PooledInner<T> can be Send if T is Sync, following the same reasoning as Pooled<T>.
unsafe impl<T: Sync> Send for PooledInner<T> {}

// SAFETY: PooledInner<T> can be Sync if T is Sync, following the same reasoning as Pooled<T>.
unsafe impl<T: Sync> Sync for PooledInner<T> {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BlindPool;

    #[test]
    fn clone_handles() {
        let pool = BlindPool::new();

        let value_handle = pool.insert(42_u64);
        let cloned_handle = value_handle.clone();

        // Both handles refer to the same value
        assert_eq!(*value_handle, 42);
        assert_eq!(*cloned_handle, 42);

        // Pool still has one item (not two)
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn automatic_cleanup_single_handle() {
        let pool = BlindPool::new();

        {
            let _value_handle = pool.insert(42_u64);
            assert_eq!(pool.len(), 1);
        } // value_handle is dropped here

        // Value should be automatically removed
        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());
    }

    #[test]
    fn automatic_cleanup_multiple_handles() {
        let pool = BlindPool::new();

        let value_handle1 = pool.insert(42_u64);
        let value_handle2 = value_handle1.clone();

        assert_eq!(pool.len(), 1);

        // Drop first handle
        drop(value_handle1);
        assert_eq!(pool.len(), 1); // Still alive because of second handle

        // Drop second handle
        drop(value_handle2);
        assert_eq!(pool.len(), 0); // Now removed
    }

    #[test]
    fn ptr_access() {
        let pool = BlindPool::new();

        let value_handle = pool.insert(42_u64);

        // Access the value directly through dereferencing
        assert_eq!(*value_handle, 42);

        // Access the value through the ptr() method
        let ptr = value_handle.ptr();
        // SAFETY: The pointer is valid as long as value_handle exists.
        let value = unsafe { ptr.read() };
        assert_eq!(value, 42);
    }

    #[test]
    fn erase_type_information() {
        let pool = BlindPool::new();

        let value_handle = pool.insert(42_u64);
        let typed_clone = value_handle.clone();
        let erased = value_handle.erase();

        // Verify the typed handle still works
        assert_eq!(*typed_clone, 42);

        // Automatic cleanup should still work - both handles refer to the same data
        drop(erased);
        drop(typed_clone);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn erase_with_multiple_references_works() {
        let pool = BlindPool::new();

        let value_handle = pool.insert(42_u64);
        let cloned_handle = value_handle.clone();

        // This should now work without panicking
        let erased = value_handle.erase();

        // Both handles should still work
        assert_eq!(*cloned_handle, 42);

        // Verify the erased handle is valid by ensuring cleanup works properly
        drop(erased);
        assert_eq!(*cloned_handle, 42); // Typed handle should still work

        drop(cloned_handle);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn drop_with_types_that_have_drop() {
        let pool = BlindPool::new();

        // Test with String and Vec - types that implement Drop
        let string_handle = pool.insert("hello".to_string());
        let vec_handle = pool.insert(vec![1, 2, 3, 4, 5]);

        assert_eq!(pool.len(), 2);
        assert_eq!(*string_handle, "hello");
        assert_eq!(*vec_handle, vec![1, 2, 3, 4, 5]);

        drop(string_handle);
        drop(vec_handle);

        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn works_with_single_byte_type() {
        let pool = BlindPool::new();

        let byte_handle = pool.insert(42_u8);
        assert_eq!(*byte_handle, 42);
        assert_eq!(pool.len(), 1);

        drop(byte_handle);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn string_methods_through_deref() {
        let pool = BlindPool::new();

        let string_handle = pool.insert("hello world".to_string());

        // Test that we can call String methods directly
        assert_eq!(string_handle.len(), 11);
        assert!(string_handle.starts_with("hello"));
        assert!(string_handle.ends_with("world"));
        assert_eq!(string_handle.chars().count(), 11);
    }

    #[test]
    fn static_assertions() {
        use std::cell::RefCell;
        use std::rc::Rc;

        use static_assertions::{assert_impl_all, assert_not_impl_any};

        // Pooled<T> should be Send if and only if T is Sync
        assert_impl_all!(Pooled<u32>: Send);
        assert_impl_all!(Pooled<String>: Send);
        assert_impl_all!(Pooled<Vec<u8>>: Send);
        assert_not_impl_any!(Pooled<RefCell<u32>>: Send); // RefCell is not Sync
        assert_not_impl_any!(Pooled<Rc<u32>>: Send); // Rc is not Sync

        // Pooled<T> should be Sync if and only if T is Sync
        assert_impl_all!(Pooled<u32>: Sync);
        assert_impl_all!(Pooled<String>: Sync);
        assert_impl_all!(Pooled<Vec<u8>>: Sync);
        assert_not_impl_any!(Pooled<RefCell<u32>>: Sync); // RefCell is not Sync
        assert_not_impl_any!(Pooled<Rc<u32>>: Sync); // Rc is not Sync

        // Pooled<T> should always be Unpin regardless of T
        assert_impl_all!(Pooled<u32>: Unpin);
        assert_impl_all!(Pooled<String>: Unpin);
        assert_impl_all!(Pooled<Vec<u8>>: Unpin);
        assert_impl_all!(Pooled<RefCell<u32>>: Unpin);
        assert_impl_all!(Pooled<Rc<u32>>: Unpin);

        // Even with non-Unpin types, Pooled should still be Unpin
        use std::marker::PhantomPinned;
        assert_impl_all!(Pooled<PhantomPinned>: Unpin);
    }
}
