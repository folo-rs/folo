use std::fmt;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;

use crate::coordinates::ItemCoordinates;

/// Shared handle to a pooled item that allows multiple references to the same value.
///
/// `Pooled<T>` provides shared access to items stored in an [`OpaquePool`] and can be copied
/// and cloned freely. This is the "shared reference" counterpart to [`crate::PooledMut<T>`]'s
/// "exclusive ownership" model, enabling flexible access patterns for pooled data.
///
/// Multiple `Pooled<T>` handles can refer to the same stored value simultaneously, making
/// this type ideal for scenarios where you need to share access to pooled data across
/// different parts of your code.
///
/// # Key Features
///
/// - **Copyable handles**: Implements [`Copy`] and [`Clone`] for easy duplication
/// - **Shared access**: Multiple handles can reference the same pooled value
/// - **Direct value access**: Implements [`std::ops::Deref`] for transparent access
/// - **Type safety**: Generic parameter `T` provides compile-time type checking
/// - **Type erasure**: Use [`erase()`](Pooled::erase) to convert to `Pooled<()>`
/// - **Pinning support**: Safe [`std::pin::Pin`] access via [`as_pin()`](Pooled::as_pin)
/// - **Pointer access**: Raw pointer access via [`ptr()`](Pooled::ptr) for advanced use cases
///
/// # Relationship with `PooledMut<T>`
///
/// `Pooled<T>` handles are typically created by converting from [`crate::PooledMut<T>`] using
/// [`into_shared()`](crate::PooledMut::into_shared). While [`crate::PooledMut<T>`] provides exclusive
/// ownership with safe removal, `Pooled<T>` trades some safety for flexibility by allowing
/// multiple references to the same value.
///
/// # Safety Considerations
///
/// Unlike [`crate::PooledMut<T>`], removing items via `Pooled<T>` requires `unsafe` code because
/// the compiler cannot prevent use-after-free when multiple handles to the same value exist.
/// The caller must ensure no other copies of the handle are used after removal.
///
/// # Examples
///
/// ## Basic shared access
///
/// ```rust
/// use opaque_pool::OpaquePool;
///
/// let mut pool = OpaquePool::builder().layout_of::<String>().build();
///
/// // SAFETY: String matches the layout used to create the pool
/// let exclusive = unsafe { pool.insert("Hello".to_string()) };
///
/// // Convert to shared handle for copying
/// let shared = exclusive.into_shared();
/// let shared_copy = shared; // Can copy freely
/// let shared_clone = shared.clone();
///
/// // All handles refer to the same value and support Deref
/// assert_eq!(&*shared_copy, "Hello");
/// assert_eq!(&*shared_clone, "Hello");
/// assert_eq!(shared_copy.len(), 5);
///
/// // Removal requires unsafe - caller ensures no other copies are used
/// // SAFETY: No other copies of the handle will be used after this call
/// unsafe { pool.remove(&shared_copy) };
/// ```
///
/// ## Type erasure and casting
///
/// ```rust
/// use opaque_pool::OpaquePool;
///
/// let mut pool = OpaquePool::builder().layout_of::<String>().build();
///
/// // SAFETY: String matches the layout used to create the pool
/// let item = unsafe { pool.insert("Hello".to_string()) }.into_shared();
///
/// // Erase type information
/// let erased = item.erase();
///
/// // Both refer to the same value
/// // SAFETY: Both pointers are valid and point to the same String
/// let original_ptr = item.ptr().as_ptr() as *const ();
/// let erased_ptr = erased.ptr().as_ptr();
/// assert_eq!(original_ptr, erased_ptr);
///
/// // SAFETY: No other copies will be used after removal
/// unsafe { pool.remove(&erased) };
/// ```
///
/// # Thread Safety
///
/// This type is thread-safe ([`Send`] + [`Sync`]) if and only if `T` implements [`Sync`].
/// When `T` is [`Sync`], multiple threads can safely share handles to the same data.
/// When `T` is not [`Sync`], the handle cannot be moved between threads or shared between
/// threads, preventing invalid access to non-thread-safe data.
///
/// [`OpaquePool`]: crate::OpaquePool
/// [`DropPolicy`]: crate::DropPolicy
pub struct Pooled<T: ?Sized> {
    /// Ensures this handle can only be returned to the pool it came from.
    pub(crate) pool_id: u64,

    pub(crate) coordinates: ItemCoordinates,

    pub(crate) ptr: NonNull<T>,
}

impl<T: ?Sized> Pooled<T> {
    /// Creates a new `Pooled<T>` handle.
    ///
    /// This method is intended for internal use by the pool implementation.
    #[must_use]
    pub(crate) fn new(pool_id: u64, coordinates: ItemCoordinates, ptr: NonNull<T>) -> Self {
        Self {
            pool_id,
            coordinates,
            ptr,
        }
    }

    /// Casts this `Pooled<T>` to a different type using a casting function.
    ///
    /// This method allows casting the pooled value to a trait object or other compatible type.
    /// The underlying memory layout and pool management remain unchanged.
    ///
    /// This method is only intended for use by the [`define_pooled_dyn_cast!`] macro
    /// and by the blind_pool crate for type-safe casting operations.
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that the handle points to a valid item that
    /// has not been removed from the pool.
    ///
    /// The caller must guarantee that the target object is in a state where it is
    /// valid to create a shared reference to it (i.e. no concurrent `&mut` exclusive
    /// references exist.)
    ///
    /// The caller must guarantee that the callback input and output references
    /// point to the same object.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    /// use std::fmt::Display;
    ///
    /// use opaque_pool::OpaquePool;
    ///
    /// let mut pool = OpaquePool::builder().layout_of::<String>().build();
    /// // SAFETY: String matches the layout used to create the pool.
    /// let value_handle = unsafe { pool.insert("Test".to_string()) }.into_shared();
    ///
    /// // Cast to trait object.
    /// let display_handle: opaque_pool::Pooled<dyn Display> =
    ///     unsafe { value_handle.__private_cast_dyn_with_fn(|x| x as &dyn Display) };
    ///
    /// // Can access as Display trait object using Deref.
    /// assert_eq!(format!("{}", &*display_handle), "Test");
    /// ```
    #[must_use]
    #[inline]
    #[doc(hidden)]
    pub unsafe fn __private_cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> Pooled<U>
    where
        F: FnOnce(&T) -> &U,
    {
        // Get a reference to perform the cast and obtain the trait object pointer.
        // SAFETY: Forwarding safety requirements to the caller.
        let value_ref = unsafe { self.ptr.as_ref() };

        // Use the provided function to perform the cast - this ensures type safety.
        let new_ref: &U = cast_fn(value_ref);

        // Now we need a pointer to stuff into a new Pooled.
        let new_ptr = NonNull::from(new_ref);

        // Create a new pooled handle with the trait object type. We can reuse the same
        // pool_id and coordinates since they refer to the same underlying allocation, just
        // with a different type view.
        Pooled {
            pool_id: self.pool_id,
            coordinates: self.coordinates,
            ptr: new_ptr,
        }
    }

    /// Converts this pooled item handle to a trait object using a mutable reference cast.
    ///
    /// This method is intended for use by `PooledMut<T>` types that need to maintain exclusive
    /// access during casting operations. It takes a casting function that works with mutable
    /// references instead of shared references.
    ///
    /// The method exists alongside the shared reference version to maintain proper borrowing
    /// semantics for different handle types - `Pooled<T>` uses shared references while
    /// `PooledMut<T>` uses exclusive references.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the handle belongs to the same pool that this
    /// item was inserted into and that the item pointed to by this handle
    /// has not been removed from the pool.
    ///
    /// The caller must guarantee that the target object is in a state where it is
    /// valid to create an exclusive reference to it (i.e. no concurrent `&` shared
    /// or `&mut` exclusive references exist).
    ///
    /// The caller must guarantee that the callback input and output references
    /// point to the same object.
    #[must_use]
    #[inline]
    #[doc(hidden)]
    pub unsafe fn __private_cast_dyn_with_fn_mut<U: ?Sized, F>(mut self, cast_fn: F) -> Pooled<U>
    where
        F: FnOnce(&mut T) -> &mut U,
    {
        // Get a mutable reference to perform the cast and obtain the trait object pointer.
        // SAFETY: Forwarding safety requirements to the caller.
        let value_ref = unsafe { self.ptr.as_mut() };

        // Use the provided function to perform the cast - this ensures type safety.
        let new_ref: &mut U = cast_fn(value_ref);

        // Now we need a pointer to stuff into a new Pooled.
        let new_ptr = NonNull::from(new_ref);

        // Create a new pooled handle with the trait object type. We can reuse the same
        // pool_id and coordinates since they refer to the same underlying allocation, just
        // with a different type view.
        Pooled {
            pool_id: self.pool_id,
            coordinates: self.coordinates,
            ptr: new_ptr,
        }
    }

    /// Returns a pointer to the inserted value.
    ///
    /// This is the only way to access the value stored in the pool. The owner of the handle has
    /// exclusive access to the value and may both read and write and may create both `&` shared
    /// and `&mut` exclusive references to the item.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use opaque_pool::OpaquePool;
    ///
    /// let mut pool = OpaquePool::builder().layout_of::<String>().build();
    ///
    /// // SAFETY: String matches the layout used to create the pool.
    /// let pooled = unsafe { pool.insert("Hello".to_string()) };
    ///
    /// // Access data using Deref.
    /// assert_eq!(&*pooled, "Hello");
    /// ```
    #[must_use]
    #[inline]
    pub fn ptr(&self) -> NonNull<T> {
        self.ptr
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
    /// use opaque_pool::OpaquePool;
    ///
    /// let mut pool = OpaquePool::builder().layout_of::<String>().build();
    ///
    /// // SAFETY: String matches the layout used to create the pool.
    /// let handle = unsafe { pool.insert("hello".to_string()) }.into_shared();
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

    /// Erases the type information from this [`Pooled<T>`] handle, returning a [`Pooled<()>`].
    ///
    /// This is useful when you want to store handles of different types in the same collection
    /// or pass them to code that doesn't need to know the specific type.
    ///
    /// The handle remains functionally equivalent and can still be used to remove the item
    /// from the pool and drop it. The only change is the removal of the type information.
    ///
    /// # Example
    ///
    /// ```rust
    /// use std::alloc::Layout;
    ///
    /// use opaque_pool::OpaquePool;
    ///
    /// let mut pool = OpaquePool::builder().layout_of::<String>().build();
    ///
    /// // SAFETY: String matches the layout used to create the pool.
    /// let pooled = unsafe { pool.insert("Test".to_string()) }.into_shared();
    ///
    /// // Erase type information.
    /// let erased = pooled.erase();
    ///
    /// // Cast back to original type for safe access.
    /// // SAFETY: We know this contains a String.
    /// let typed_ptr = erased.ptr().cast::<String>();
    /// let value = unsafe { typed_ptr.as_ref() };
    /// assert_eq!(value, "Test");
    ///
    /// // Can still remove the item.
    /// unsafe { pool.remove(&erased) };
    /// ```
    #[must_use]
    #[inline]
    pub fn erase(self) -> Pooled<()> {
        Pooled {
            pool_id: self.pool_id,
            coordinates: self.coordinates,
            ptr: self.ptr.cast::<()>(),
        }
    }
}

impl<T: ?Sized> Deref for Pooled<T> {
    type Target = T;

    /// Provides direct access to the value stored in the pool.
    ///
    /// This allows the handle to be used as if it were a reference to the stored value.
    /// Since `Pooled<T>` provides shared access, this returns a shared reference.
    ///
    /// # Example
    ///
    /// ```rust
    /// use opaque_pool::OpaquePool;
    ///
    /// let mut pool = OpaquePool::builder().layout_of::<String>().build();
    ///
    /// // SAFETY: String matches the layout used to create the pool.
    /// let string_handle = unsafe { pool.insert("hello".to_string()) }.into_shared();
    ///
    /// // Access string methods directly.
    /// assert_eq!(string_handle.len(), 5);
    /// assert!(string_handle.starts_with("he"));
    /// ```
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: The pooled handle is valid and contains initialized memory of type T.
        // The handle ensures the underlying pool data remains alive during access.
        unsafe { self.ptr.as_ref() }
    }
}

impl<T: ?Sized> Copy for Pooled<T> {}

impl<T: ?Sized> Clone for Pooled<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> fmt::Debug for Pooled<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pooled")
            .field("type_name", &std::any::type_name::<T>())
            .field("pool_id", &self.pool_id)
            .field("coordinates", &self.coordinates)
            .field("ptr", &self.ptr)
            .finish()
    }
}

// SAFETY: Pooled<T> is just a fancy reference, so its thread-safety is entirely driven by the
// underlying type T and the presence of the `Sync` auto trait on it.
unsafe impl<T: ?Sized + Sync> Send for Pooled<T> {}

// SAFETY: Pooled<T> is just a fancy reference, so its thread-safety is entirely driven by the
// underlying type T and the presence of the `Sync` auto trait on it.
unsafe impl<T: ?Sized + Sync> Sync for Pooled<T> {}

#[cfg(test)]
#[allow(
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block,
    reason = "tests focus on succinct code and do not need to tick all the boxes"
)]
mod tests {
    use crate::OpaquePool;

    #[test]
    fn deref() {
        let mut pool = OpaquePool::builder().layout_of::<String>().build();

        let pooled_mut = unsafe { pool.insert("Shared access test".to_string()) };
        let pooled = pooled_mut.into_shared();

        // Test Deref trait for shared access
        assert_eq!(&*pooled, "Shared access test");
        assert_eq!(pooled.len(), 18);
        assert!(pooled.contains("access"));

        // Verify we can use string methods through deref
        let uppercase = pooled.to_uppercase();
        assert_eq!(uppercase, "SHARED ACCESS TEST");

        unsafe {
            pool.remove(&pooled);
        }
    }

    #[test]
    fn as_pin() {
        let mut pool = OpaquePool::builder().layout_of::<String>().build();

        let pooled_mut = unsafe { pool.insert("Shared pin test".to_string()) };
        let pooled = pooled_mut.into_shared();

        // Test as_pin() method for shared access
        let pinned_ref = pooled.as_pin();
        assert_eq!(&**pinned_ref, "Shared pin test");

        // Verify we can use Pin methods
        let len = pinned_ref.len();
        assert_eq!(len, 15);

        unsafe {
            pool.remove(&pooled);
        }
    }

    #[test]
    fn erase_functionality() {
        use std::alloc::Layout;

        let layout = Layout::new::<String>();
        let mut pool = OpaquePool::builder().layout(layout).build();

        let pooled_mut = unsafe { pool.insert("Test".to_string()) };
        let pooled = pooled_mut.into_shared();

        // Test that the typed pointer works.
        unsafe {
            assert_eq!(pooled.ptr().as_ref(), "Test");
        }

        // Erase the type information.
        let erased = pooled.erase();

        // Should still be able to access the value through the erased pointer.
        unsafe {
            assert_eq!(erased.ptr().cast::<String>().as_ref(), "Test");
        }

        // Should be able to remove the erased handle.
        unsafe {
            pool.remove(&erased);
        }
    }
}
