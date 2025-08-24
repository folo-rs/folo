use std::fmt;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::ptr::NonNull;

use crate::{ItemCoordinates, Pooled};

/// Exclusive handle to a pooled item that prevents double-use through the type system.
///
/// `PooledMut<T>` provides exclusive ownership of an item stored in an [`OpaquePool`] and
/// ensures that each handle can only be used once. Unlike [`Pooled<T>`], this type does not
/// implement [`Copy`] or [`Clone`], making it impossible to accidentally create multiple
/// handles to the same pooled item or use a handle after it has been consumed.
///
/// This is the recommended handle type for most use cases as it provides stronger safety
/// guarantees and enables safe removal operations without requiring `unsafe` code.
///
/// # Key Features
///
/// - **Exclusive ownership**: Each handle represents unique access to one pooled item
/// - **Single-use guarantee**: Cannot be copied, cloned, or reused after consumption
/// - **Safe removal**: Removal methods consume the handle, preventing use-after-free
/// - **Direct value access**: Implements [`std::ops::Deref`] and [`std::ops::DerefMut`]
/// - **Mutable access**: Full read-write access to the stored value
/// - **Conversion to shared**: Use [`into_shared()`](PooledMut::into_shared) to enable copying
/// - **Pinning support**: Safe [`std::pin::Pin`] access for both `&T` and `&mut T`
/// - **Type erasure**: Use [`erase()`](PooledMut::erase) to convert to `PooledMut<()>`
/// - **Pointer access**: Raw pointer access via [`ptr()`](PooledMut::ptr) for advanced cases
///
/// # Safety Benefits
///
/// `PooledMut<T>` provides stronger compile-time guarantees than [`Pooled<T>`]:
///
/// - **No double-use**: Cannot be copied or cloned, preventing accidental reuse
/// - **Safe removal**: Pool removal methods consume the handle, making reuse impossible
/// - **Move semantics**: Handle is moved/consumed by operations, enforcing single use
/// - **Type system enforcement**: Rust's ownership system prevents use-after-move errors
///
/// # Relationship with `Pooled<T>`
///
/// `PooledMut<T>` can be converted to [`Pooled<T>`] using [`into_shared()`](PooledMut::into_shared)
/// when you need to share access across multiple references. This conversion is one-way;
/// you cannot convert back from [`Pooled<T>`] to `PooledMut<T>`.
///
/// # Examples
///
/// ## Safe exclusive access and removal
///
/// ```rust
/// use opaque_pool::OpaquePool;
///
/// let mut pool = OpaquePool::builder().layout_of::<String>().build();
///
/// // Insert and get exclusive handle
/// // SAFETY: String matches the layout used to create the pool
/// let mut item = unsafe { pool.insert("Hello".to_string()) };
///
/// // Direct access through Deref and DerefMut
/// assert_eq!(&*item, "Hello");
/// item.push_str(", World!");
/// assert_eq!(&*item, "Hello, World!");
///
/// // Safe removal - handle is consumed, preventing reuse
/// pool.remove_mut(item);
/// // item is now moved and cannot be used again!
/// ```
///
/// ## Converting to shared access
///
/// ```rust
/// use opaque_pool::OpaquePool;
///
/// let mut pool = OpaquePool::builder().layout_of::<u64>().build();
///
/// // SAFETY: u64 matches the layout used to create the pool
/// let exclusive = unsafe { pool.insert(42_u64) };
///
/// // Convert to shared handle for copying
/// let shared = exclusive.into_shared();
/// let shared_copy = shared; // Now can copy freely
///
/// assert_eq!(*shared_copy, 42);
///
/// // Removal now requires unsafe since multiple handles could exist
/// // SAFETY: No other copies of the handle will be used after this call
/// unsafe { pool.remove(&shared_copy) };
/// ```
///
/// ## Pinning support
///
/// ```rust
/// use std::pin::Pin;
///
/// use opaque_pool::OpaquePool;
///
/// let mut pool = OpaquePool::builder().layout_of::<String>().build();
///
/// // SAFETY: String matches the layout used to create the pool
/// let item = unsafe { pool.insert("Pinned".to_string()) };
///
/// // Get pinned references safely
/// let pinned_ref: Pin<&String> = item.as_pin();
/// assert_eq!(&**pinned_ref, "Pinned");
///
/// pool.remove_mut(item);
/// ```
///
/// # Thread Safety
///
/// This type has the same thread safety properties as [`Pooled<T>`]. It is thread-safe
/// ([`Send`] + [`Sync`]) if and only if `T` implements [`Sync`]. When `T` is [`Sync`],
/// the handle can be moved between threads or shared between threads safely.
///
/// [`OpaquePool`]: crate::OpaquePool
pub struct PooledMut<T: ?Sized> {
    /// Ensures this handle can only be returned to the pool it came from.
    pub(crate) pool_id: u64,

    pub(crate) coordinates: ItemCoordinates,

    pub(crate) ptr: NonNull<T>,
}

impl<T: ?Sized> PooledMut<T> {
    /// Creates a new `PooledMut<T>` handle.
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

    /// Returns a pointer to the inserted value.
    ///
    /// This provides access to the value stored in the pool. The owner of the handle has
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
    /// let mut item = unsafe { pool.insert("Hello".to_string()) };
    ///
    /// // Read data back using Deref.
    /// assert_eq!(&*item, "Hello");
    ///
    /// // Can also write to it using DerefMut.
    /// item.push_str(", World!");
    /// assert_eq!(&*item, "Hello, World!");
    /// ```
    #[must_use]
    #[inline]
    pub fn ptr(&self) -> NonNull<T> {
        self.ptr
    }

    /// Converts this exclusive handle to a shared handle.
    ///
    /// This allows multiple shared handles to exist to the same pooled item.
    /// The returned [`Pooled<T>`] handle can be copied and shared, but removal
    /// operations will require `unsafe` code again.
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
    /// let item = unsafe { pool.insert("Test".to_string()) };
    ///
    /// // Convert to shared handle.
    /// let shared = item.into_shared();
    ///
    /// // Can now copy the handle.
    /// let shared_copy = shared;
    ///
    /// // But removal requires unsafe again.
    /// unsafe { pool.remove(&shared_copy) };
    /// ```
    #[must_use]
    #[inline]
    pub fn into_shared(self) -> Pooled<T> {
        Pooled::new(self.pool_id, self.coordinates, self.ptr)
    }

    /// Erases the type information from this [`PooledMut<T>`] handle, returning a [`PooledMut<()>`].
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
    /// let item = unsafe { pool.insert("Test".to_string()) };
    ///
    /// // Erase type information.
    /// let erased = item.erase();
    ///
    /// // Cast back to original type for safe access.
    /// // SAFETY: We know this contains a String.
    /// let typed_ptr = erased.ptr().cast::<String>();
    /// let value = unsafe { typed_ptr.as_ref() };
    /// assert_eq!(value, "Test");
    ///
    /// // Can still remove the item.
    /// pool.remove_mut(erased);
    /// ```
    #[must_use]
    #[inline]
    pub fn erase(self) -> PooledMut<()> {
        PooledMut {
            pool_id: self.pool_id,
            coordinates: self.coordinates,
            ptr: self.ptr.cast::<()>(),
        }
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
    /// let handle = unsafe { pool.insert("hello".to_string()) };
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

    /// Returns a pinned mutable reference to the value stored in the pool.
    ///
    /// Since values in the pool are always pinned (they never move once inserted),
    /// this method provides safe access to `Pin<&mut T>` without requiring unsafe code.
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
    /// let mut handle = unsafe { pool.insert("hello".to_string()) };
    ///
    /// let mut pinned: Pin<&mut String> = handle.as_pin_mut();
    /// // Can use Pin methods or deref to &mut String
    /// ```
    #[must_use]
    #[inline]
    pub fn as_pin_mut(&mut self) -> Pin<&mut T> {
        // SAFETY: Values in the pool are always pinned - they never move once inserted.
        // The pool ensures stable addresses for the lifetime of the pooled object.
        // We have exclusive access through &mut self, so this is safe.
        unsafe { Pin::new_unchecked(&mut **self) }
    }
}

impl<T: ?Sized> Deref for PooledMut<T> {
    type Target = T;

    /// Provides direct access to the value stored in the pool.
    ///
    /// This allows the handle to be used as if it were a reference to the stored value.
    ///
    /// # Example
    ///
    /// ```rust
    /// use opaque_pool::OpaquePool;
    ///
    /// let mut pool = OpaquePool::builder().layout_of::<String>().build();
    ///
    /// // SAFETY: String matches the layout used to create the pool.
    /// let string_handle = unsafe { pool.insert("hello".to_string()) };
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

impl<T: ?Sized> DerefMut for PooledMut<T> {
    /// Provides direct mutable access to the value stored in the pool.
    ///
    /// This allows the handle to be used as if it were a mutable reference to the stored value.
    ///
    /// # Example
    ///
    /// ```rust
    /// use opaque_pool::OpaquePool;
    ///
    /// let mut pool = OpaquePool::builder().layout_of::<String>().build();
    ///
    /// // SAFETY: String matches the layout used to create the pool.
    /// let mut string_handle = unsafe { pool.insert("hello".to_string()) };
    ///
    /// // Mutate the string directly.
    /// string_handle.push_str(" world");
    /// assert_eq!(*string_handle, "hello world");
    /// ```
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: The pooled handle is valid and contains initialized memory of type T.
        // We have exclusive access through PooledMut, so mutable access is safe.
        unsafe { self.ptr.as_mut() }
    }
}

impl<T: ?Sized> fmt::Debug for PooledMut<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PooledMut")
            .field("type_name", &std::any::type_name::<T>())
            .field("pool_id", &self.pool_id)
            .field("coordinates", &self.coordinates)
            .field("ptr", &self.ptr)
            .finish()
    }
}

// SAFETY: PooledMut<T> is just a fancy exclusive reference, so its thread-safety is entirely
// driven by the underlying type T and the presence of the `Sync` auto trait on it.
unsafe impl<T: ?Sized + Sync> Send for PooledMut<T> {}

// SAFETY: PooledMut<T> is just a fancy exclusive reference, so its thread-safety is entirely
// driven by the underlying type T and the presence of the `Sync` auto trait on it.
unsafe impl<T: ?Sized + Sync> Sync for PooledMut<T> {}

#[cfg(test)]
#[allow(
    clippy::undocumented_unsafe_blocks,
    clippy::multiple_unsafe_ops_per_block,
    clippy::items_after_statements,
    reason = "tests focus on succinct code and do not need to tick all the boxes"
)]
mod tests {
    use std::mem::MaybeUninit;

    use crate::OpaquePool;

    #[test]
    fn smoke_test() {
        let mut pool = OpaquePool::builder().layout_of::<String>().build();

        assert_eq!(pool.len(), 0);
        assert!(pool.is_empty());

        let pooled_mut_a = unsafe { pool.insert("Hello".to_string()) };
        let pooled_mut_b = unsafe { pool.insert("World".to_string()) };
        let pooled_mut_c = unsafe { pool.insert("Test".to_string()) };

        assert_eq!(pool.len(), 3);
        assert!(!pool.is_empty());
        assert!(pool.capacity() >= 3);

        unsafe {
            assert_eq!(pooled_mut_a.ptr().as_ref(), "Hello");
            assert_eq!(pooled_mut_b.ptr().as_ref(), "World");
            assert_eq!(pooled_mut_c.ptr().as_ref(), "Test");
        }

        pool.remove_mut(pooled_mut_b);

        let pooled_mut_d = unsafe { pool.insert("Updated".to_string()) };

        unsafe {
            assert_eq!(pooled_mut_a.ptr().as_ref(), "Hello");
            assert_eq!(pooled_mut_c.ptr().as_ref(), "Test");
            assert_eq!(pooled_mut_d.ptr().as_ref(), "Updated");
        }

        pool.remove_mut(pooled_mut_a);
        let extracted = pool.remove_unpin_mut(pooled_mut_d);
        assert_eq!(extracted, "Updated");
        // We do not remove pooled_mut_c, leaving that up to the pool to clean up.
    }

    #[test]
    fn into_shared() {
        let mut pool = OpaquePool::builder().layout_of::<String>().build();

        let pooled_mut = unsafe { pool.insert("Test".to_string()) };
        let shared = pooled_mut.into_shared();

        // Can copy the shared handle.
        let shared_copy = shared;

        unsafe {
            assert_eq!(shared.ptr().as_ref(), "Test");
            assert_eq!(shared_copy.ptr().as_ref(), "Test");
        }

        // Clean up with the shared handle (requires unsafe again).
        unsafe {
            pool.remove(&shared_copy);
        }
    }

    #[test]
    fn insert_with() {
        let mut pool = OpaquePool::builder().layout_of::<String>().build();

        let pooled_mut = unsafe {
            pool.insert_with(|uninit: &mut MaybeUninit<String>| {
                uninit.write(String::from("In-place initialized"));
            })
        };

        unsafe {
            assert_eq!(pooled_mut.ptr().as_ref(), "In-place initialized");
        }

        let extracted = pool.remove_unpin_mut(pooled_mut);
        assert_eq!(extracted, "In-place initialized");
    }

    #[test]
    fn erase() {
        let mut pool = OpaquePool::builder().layout_of::<String>().build();

        let pooled_mut = unsafe { pool.insert("Test".to_string()) };
        let erased = pooled_mut.erase();

        // Can still access the raw pointer.
        unsafe {
            let value = erased.ptr().cast::<String>().as_ref();
            assert_eq!(value.as_str(), "Test");
        }

        // Can still remove the item.
        pool.remove_mut(erased);
    }

    #[test]
    fn deref() {
        let mut pool = OpaquePool::builder().layout_of::<String>().build();

        let mut pooled_mut = unsafe { pool.insert("Hello, World!".to_string()) };

        // Test Deref trait - should be able to access the value directly
        assert_eq!(&*pooled_mut, "Hello, World!");
        assert_eq!(pooled_mut.len(), 13);
        assert!(pooled_mut.starts_with("Hello"));

        // Test DerefMut trait - should be able to mutate the value directly
        pooled_mut.push_str(" Testing deref!");
        assert_eq!(&*pooled_mut, "Hello, World! Testing deref!");

        // Make sure the mutation persisted in the pool
        unsafe {
            assert_eq!(pooled_mut.ptr().as_ref(), "Hello, World! Testing deref!");
        }

        pool.remove_mut(pooled_mut);
    }

    #[test]
    fn as_pin() {
        let mut pool = OpaquePool::builder().layout_of::<String>().build();

        let mut pooled_mut = unsafe { pool.insert("Pin test".to_string()) };

        // Test as_pin() method
        let pinned_ref = pooled_mut.as_pin();
        assert_eq!(&**pinned_ref, "Pin test");

        // Test as_pin_mut() method
        let pinned_mut = pooled_mut.as_pin_mut();
        // Can deref Pin<&mut T> to get &mut T
        pinned_mut.get_mut().push_str(" - pinned mutation");

        // Verify the mutation worked
        assert_eq!(&*pooled_mut, "Pin test - pinned mutation");

        pool.remove_mut(pooled_mut);
    }

    #[test]
    fn deref_coercion_works() {
        let mut pool = OpaquePool::builder().layout_of::<String>().build();

        let pooled_mut = unsafe { pool.insert("Coercion test".to_string()) };

        // Test that deref coercion works - we can pass PooledMut<String> where &str is expected
        fn take_str_ref(s: &str) -> usize {
            s.len()
        }

        let len = take_str_ref(&pooled_mut);
        assert_eq!(len, 13);

        pool.remove_mut(pooled_mut);
    }

    #[test]
    fn deref_with_custom_type() {
        #[derive(Debug, PartialEq)]
        struct CustomType {
            value: i32,
            name: String,
        }

        impl CustomType {
            fn new(value: i32, name: String) -> Self {
                Self { value, name }
            }

            fn increment(&mut self) {
                self.value += 1;
            }

            fn get_value(&self) -> i32 {
                self.value
            }
        }

        let mut pool = OpaquePool::builder().layout_of::<CustomType>().build();

        let mut pooled_mut = unsafe { pool.insert(CustomType::new(42, "test".to_string())) };

        // Test Deref with custom type
        assert_eq!(pooled_mut.get_value(), 42);
        assert_eq!(pooled_mut.name, "test");

        // Test DerefMut with custom type
        pooled_mut.increment();
        assert_eq!(pooled_mut.get_value(), 43);

        pool.remove_mut(pooled_mut);
    }

    #[test]
    fn pin_methods_work() {
        let mut pool = OpaquePool::builder().layout_of::<String>().build();

        let mut pooled_mut = unsafe { pool.insert("Pin test".to_string()) };

        // Test that as_pin() returns a proper Pin<&T>
        let pin_ref = pooled_mut.as_pin();
        assert_eq!(pin_ref.get_ref(), "Pin test");

        // Test that as_pin_mut() returns a proper Pin<&mut T>
        let pin_mut = pooled_mut.as_pin_mut();
        assert_eq!(&**pin_mut, "Pin test");

        // Test that Pin<&mut T> can be used for mutation
        let pin_mut = pooled_mut.as_pin_mut();
        pin_mut.get_mut().push_str(" - modified through pin");

        // Verify the mutation worked
        assert_eq!(&*pooled_mut, "Pin test - modified through pin");

        pool.remove_mut(pooled_mut);
    }
}
