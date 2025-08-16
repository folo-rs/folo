use std::fmt;
use std::ops::Deref;
use std::ptr::NonNull;
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
/// # Trait Objects
///
/// **Important**: To downcast to a trait object, you must use [`ptr()`][Self::ptr]
/// followed by [`as_ref()`][std::ptr::NonNull::as_ref] or [`as_mut()`][std::ptr::NonNull::as_mut].
/// The [`Deref`] trait cannot be used for trait object conversion.
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
/// // CORRECT: Use ptr().as_ref() for trait objects
/// // SAFETY: The pointer is valid and contains the value we just inserted.
/// let trait_ref: &dyn MyTrait = unsafe { handle.ptr().as_ref() };
/// trait_ref.do_something();
///
/// // WRONG: This will not work for trait object conversion
/// // let trait_ref: &dyn MyTrait = &*handle; // Compilation error!
/// ```
///
/// # Example
///
/// ```rust
/// use blind_pool::BlindPool;
///
/// let pool = BlindPool::new();
/// let value_handle = pool.insert(42_u32);
///
/// // Access the value through dereferencing.
/// assert_eq!(*value_handle, 42);
///
/// // Clone to create additional references.
/// let cloned_handle = value_handle.clone();
/// assert_eq!(*cloned_handle, 42);
/// ```
pub struct Pooled<T: ?Sized> {
    /// The reference-counted inner data containing the actual pooled item and pool handle.
    inner: Arc<PooledInner<T>>,
}

/// Internal data structure that manages the lifetime of a pooled item.
///
/// This is always type-erased to `()` and shared among all typed views of the same item.
/// It ensures that the item is removed from the pool exactly once when all references are dropped.
#[doc(hidden)]
pub struct PooledRef {
    /// The type-erased handle to the actual item in the pool.
    pooled: RawPooled<()>,

    /// A handle to the pool that keeps it alive as long as this item exists.
    pool: BlindPool,
}

/// Internal data structure that contains the typed access to the pooled item.
pub struct PooledInner<T: ?Sized> {
    /// The typed handle to the actual item in the pool.
    pooled: RawPooled<T>,

    /// A shared reference to the lifetime manager.
    lifetime: Arc<PooledRef>,
}

impl<T: ?Sized> PooledInner<T> {
    /// Creates a new PooledInner with the given components.
    ///
    /// This method is intended for internal use when reconstructing pooled values
    /// after type casting operations via the [`define_pooled_dyn_cast!`] macro.
    #[doc(hidden)]
    #[must_use]
    pub fn new(pooled: RawPooled<T>, pool: BlindPool) -> Self {
        let lifetime = Arc::new(PooledRef {
            pooled: pooled.erase(),
            pool,
        });
        Self { pooled, lifetime }
    }

    /// Creates a new PooledInner sharing the lifetime with an existing one.
    ///
    /// This is used for type casting operations where we want to create a new typed view
    /// while sharing the same lifetime management.
    #[doc(hidden)]
    #[must_use]
    pub fn with_shared_lifetime(pooled: RawPooled<T>, lifetime: Arc<PooledRef>) -> Self {
        Self { pooled, lifetime }
    }
}

impl<T: ?Sized> Pooled<T> {
    /// Creates a new pooled value.
    ///
    /// This method is intended for internal use by [`BlindPool`] and for
    /// reconstructing pooled values after type casting via the [`define_pooled_dyn_cast!`] macro.
    #[doc(hidden)]
    #[must_use]
    pub fn new(pooled: RawPooled<T>, pool: BlindPool) -> Self {
        let inner = PooledInner::new(pooled, pool);
        Self {
            inner: Arc::new(inner),
        }
    }

    /// Returns a pointer to the inserted value.
    ///
    /// This provides direct access to the value stored in the pool. The caller must ensure
    /// that Rust's aliasing rules are respected when using this pointer.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    ///
    /// let pool = BlindPool::new();
    /// let value_handle = pool.insert(42_u64);
    ///
    /// let ptr = value_handle.ptr();
    ///
    /// // SAFETY: The pointer is valid and contains the value we just inserted.
    /// let value = unsafe { ptr.read() };
    /// assert_eq!(value, 42);
    /// ```
    #[must_use]
    #[inline]
    pub fn ptr(&self) -> NonNull<T> {
        self.inner.pooled.ptr()
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
    /// let value_handle = pool.insert(42_u64);
    /// let cloned_handle = value_handle.clone();
    ///
    /// // Erase type information while keeping the original handle via clone.
    /// let erased = value_handle.erase();
    ///
    /// // Both handles are valid and refer to the same item.
    /// assert_eq!(*cloned_handle, 42);
    ///
    /// // Can still access the raw pointer.
    /// // SAFETY: We know this contains a u64.
    /// let value = unsafe { erased.ptr().cast::<u64>().read() };
    /// assert_eq!(value, 42);
    /// ```
    #[must_use]
    pub fn erase(self) -> Pooled<()> {
        // Create a new erased handle sharing the same lifetime manager
        let erased_pooled = self.inner.pooled.erase();
        let erased_inner =
            PooledInner::with_shared_lifetime(erased_pooled, Arc::clone(&self.inner.lifetime));

        Pooled {
            inner: Arc::new(erased_inner),
        }
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
    /// This method is primarily intended for use by the [`define_pooled_dyn_cast!`] macro.
    /// For most use cases, prefer the type-safe cast methods generated by that macro.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::BlindPool;
    /// use std::fmt::Display;
    ///
    /// let pool = BlindPool::new();
    /// let value_handle = pool.insert(42_u64);
    /// let cloned_handle = value_handle.clone();
    ///
    /// // Cast to trait object while keeping the original handle via clone.
    /// let display_handle: blind_pool::Pooled<dyn Display> =
    ///     value_handle.cast_dyn_with_fn(|x| x as &dyn Display);
    ///
    /// // Both handles are valid and refer to the same item.
    /// assert_eq!(*cloned_handle, 42);
    /// assert_eq!(format!("{}", &*display_handle), "42");
    /// ```
    /// ```
    #[must_use]
    #[doc(hidden)]
    pub fn cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> Pooled<U>
    where
        F: FnOnce(&T) -> &U,
    {
        // Cast the RawPooled to the trait object using the provided function
        let cast_pooled = self.inner.pooled.cast_dyn_with_fn(cast_fn);
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
    /// let value_handle = pool.insert(42_u64);
    ///
    /// let cloned_handle = value_handle.clone();
    ///
    /// // Both handles refer to the same value.
    /// assert_eq!(*value_handle, *cloned_handle);
    ///
    /// // Value remains in pool until all handles are dropped.
    /// drop(value_handle);
    /// assert_eq!(*cloned_handle, 42); // Still accessible.
    /// ```
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
    fn deref(&self) -> &Self::Target {
        // SAFETY: The pooled handle is valid and contains initialized memory of type T.
        // The Arc reference count ensures the underlying pool data remains alive during access.
        unsafe { self.inner.pooled.ptr().as_ref() }
    }
}

impl Drop for PooledRef {
    /// Automatically removes the item from the pool when the last reference is dropped.
    ///
    /// This ensures that resources are properly cleaned up without requiring manual intervention.
    fn drop(&mut self) {
        // We are guaranteed to be the only one executing this drop because Arc ensures
        // that Drop on the PooledRef is only called once when the last reference is released.
        self.pool.remove(self.pooled);
    }
}

// SAFETY: Pooled<T> can be Send if T is Send, because the Arc<PooledInner<T>>
// is Send when T is Send, and the mutex in BlindPool provides thread safety.
unsafe impl<T: Send> Send for Pooled<T> {}

// SAFETY: Pooled<T> can be Sync if T is Sync, because multiple threads can safely
// access the same Pooled<T> instance if T is Sync. The deref operation is safe
// for concurrent access when T is Sync, and other operations don't require exclusive access.
unsafe impl<T: Sync> Sync for Pooled<T> {}

impl<T: fmt::Debug> fmt::Debug for Pooled<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pooled")
            .field("inner", &self.inner)
            .finish()
    }
}

impl<T: fmt::Debug> fmt::Debug for PooledInner<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PooledInner")
            .field("pooled", &self.pooled)
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

// SAFETY: PooledInner<T> can be Send if T is Send, following the same reasoning as Pooled<T>.
unsafe impl<T: Send> Send for PooledInner<T> {}

// SAFETY: PooledInner<T> can be Sync if T is Sync, following the same reasoning as Pooled<T>.
unsafe impl<T: Sync> Sync for PooledInner<T> {}
