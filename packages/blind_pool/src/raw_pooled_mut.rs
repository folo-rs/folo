use std::alloc::Layout;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::ptr::NonNull;

use opaque_pool::PooledMut;

use crate::RawPooled;

/// A handle to a value stored in a [`super::RawBlindPool`] with exclusive ownership guarantees.
///
/// Unlike [`RawPooled<T>`], this handle cannot be copied or cloned, ensuring that only one
/// handle can exist for each pool item. This enables safe removal methods that consume the
/// handle, preventing double-use bugs that could lead to undefined behavior.
///
/// The handle provides access to the stored value via a pointer, similar to [`RawPooled<T>`],
/// but with the additional safety guarantee of exclusive ownership.
///
/// # Thread safety
///
/// When `T` is [`Sync`], the handle is thread-safe and can be freely moved and shared between
/// threads, providing safe concurrent access to the underlying data.
///
/// When `T` is not [`Sync`], the handle is single-threaded and cannot be moved between threads
/// or shared between threads, preventing unsafe access to non-thread-safe data.
pub struct RawPooledMut<T: ?Sized> {
    /// The memory layout of the stored item. This is used to identify which internal
    /// pool the item belongs to.
    pub(crate) layout: Layout,

    /// The exclusive handle from the internal opaque pool.
    pub(crate) inner: PooledMut<T>,
}

impl<T: ?Sized> RawPooledMut<T> {
    /// Returns a pointer to the inserted value.
    ///
    /// This is the only way to access the value stored in the pool. The owner of the handle has
    /// exclusive access to the value and may both read and write and may create both `&` shared
    /// and `&mut` exclusive references to the item.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    ///
    /// let pooled_mut = pool.insert_mut(2.5159_f64);
    ///
    /// // Read data back from the memory.
    /// let value = *pooled_mut; // Safe deref access
    /// assert_eq!(value, 2.5159);
    /// ```
    #[must_use]
    #[inline]
    pub fn ptr(&self) -> NonNull<T> {
        self.inner.ptr()
    }

    /// Erases the type information from this `RawPooledMut<T>` handle,
    /// returning a `RawPooledMut<()>`.
    ///
    /// This is useful when you want to store handles of different types in the same collection
    /// or pass them to code that doesn't need to know the specific type.
    ///
    /// The handle remains functionally equivalent and can still be used to remove the item
    /// from the pool safely. The only change is the removal of the type information.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    /// let pooled_mut = pool.insert_mut("Test".to_string());
    ///
    /// // Erase the type information.
    /// let erased = pooled_mut.erase();
    ///
    /// // Can still be removed safely.
    /// pool.remove_mut(erased);
    /// ```
    #[must_use]
    #[inline]
    pub fn erase(self) -> RawPooledMut<()> {
        RawPooledMut {
            layout: self.layout,
            inner: self.inner.erase(),
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
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    /// let handle = pool.insert_mut("hello".to_string());
    ///
    /// let pinned: Pin<&String> = handle.as_pin();
    /// assert_eq!(pinned.len(), 5);
    /// ```
    #[must_use]
    #[inline]
    pub fn as_pin(&self) -> Pin<&T> {
        self.inner.as_pin()
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
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    /// let mut handle = pool.insert_mut("hello".to_string());
    ///
    /// let mut pinned: Pin<&mut String> = handle.as_pin_mut();
    /// // Can use Pin methods or deref to &mut String
    /// ```
    #[must_use]
    #[inline]
    pub fn as_pin_mut(&mut self) -> Pin<&mut T> {
        self.inner.as_pin_mut()
    }

    /// Converts this exclusive handle to a shared handle.
    ///
    /// This consumes the `RawPooledMut<T>` and returns a `RawPooled<T>` that can be copied
    /// and cloned. Use this when you no longer need the exclusive ownership guarantees
    /// and want to share the handle.
    ///
    /// Note that after calling this method, you lose the safety guarantees of exclusive
    /// ownership and must use unsafe removal methods.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    /// let pooled_mut = pool.insert_mut("Test".to_string());
    ///
    /// // Convert to shared handle.
    /// let shared = pooled_mut.into_shared();
    ///
    /// // Can now copy the handle.
    /// let shared_copy = shared;
    ///
    /// // But removal requires unsafe code again.
    /// unsafe { pool.remove(&shared_copy) };
    /// ```
    #[must_use]
    #[inline]
    pub fn into_shared(self) -> RawPooled<T> {
        RawPooled {
            layout: self.layout,
            inner: self.inner.into_shared(),
        }
    }
}

impl<T: ?Sized> Deref for RawPooledMut<T> {
    type Target = T;

    /// Provides direct access to the value stored in the pool.
    ///
    /// This allows the handle to be used as if it were a reference to the stored value.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    /// let string_handle = pool.insert_mut("hello".to_string());
    ///
    /// // Access string methods directly.
    /// assert_eq!(string_handle.len(), 5);
    /// assert!(string_handle.starts_with("he"));
    /// ```
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: The inner handle is valid and contains initialized memory of type T.
        unsafe { self.inner.ptr().as_ref() }
    }
}

impl<T: ?Sized> DerefMut for RawPooledMut<T> {
    /// Provides direct mutable access to the value stored in the pool.
    ///
    /// This allows the handle to be used as if it were a mutable reference to the stored value.
    ///
    /// # Example
    ///
    /// ```rust
    /// use blind_pool::RawBlindPool;
    ///
    /// let mut pool = RawBlindPool::new();
    /// let mut string_handle = pool.insert_mut("hello".to_string());
    ///
    /// // Mutate the string directly.
    /// string_handle.push_str(" world");
    /// assert_eq!(*string_handle, "hello world");
    /// ```
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: The inner handle is valid and contains initialized memory of type T.
        // We have exclusive access through PooledMut, so mutable access is safe.
        unsafe { self.inner.ptr().as_mut() }
    }
}

impl<T: ?Sized> fmt::Debug for RawPooledMut<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawPooledMut")
            .field("layout", &self.layout)
            .field("inner", &self.inner)
            .finish()
    }
}

// SAFETY: RawPooledMut<T> is just a fancy reference with exclusive ownership, so its thread-safety
// is entirely driven by the underlying type T and the presence of the `Sync` auto trait on it.
unsafe impl<T: ?Sized + Sync> Send for RawPooledMut<T> {}

// SAFETY: RawPooledMut<T> is just a fancy reference with exclusive ownership, so its thread-safety
// is entirely driven by the underlying type T and the presence of the `Sync` auto trait on it.
unsafe impl<T: ?Sized + Sync> Sync for RawPooledMut<T> {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RawBlindPool;

    #[test]
    fn ptr_access() {
        let mut pool = RawBlindPool::new();
        let pooled_mut = pool.insert_mut(42_u32);

        let value = *pooled_mut; // Safe deref access
        assert_eq!(value, 42);

        pool.remove_mut(pooled_mut);
    }

    #[test]
    fn ptr_mut_access() {
        let mut pool = RawBlindPool::new();
        let pooled_mut = pool.insert_mut(42_u32);

        // SAFETY: The pointer is valid and we have exclusive access.
        unsafe {
            let ptr = pooled_mut.ptr();
            ptr.write(84);
        }

        let value = *pooled_mut; // Safe deref access
        assert_eq!(value, 84);

        pool.remove_mut(pooled_mut);
    }

    #[test]
    fn deref_functionality() {
        let mut pool = RawBlindPool::new();
        let mut pooled_mut = pool.insert_mut("Hello".to_string());

        // Test Deref - should be able to access the value directly
        assert_eq!(&*pooled_mut, "Hello");
        assert_eq!(pooled_mut.len(), 5);
        assert!(pooled_mut.starts_with("He"));

        // Test DerefMut - should be able to mutate the value directly
        pooled_mut.push_str(", World!");
        assert_eq!(&*pooled_mut, "Hello, World!");

        pool.remove_mut(pooled_mut);
    }

    #[test]
    fn deref_coercion_works() {
        // Function that takes &str.
        fn char_count(s: &str) -> usize {
            s.chars().count()
        }

        let mut pool = RawBlindPool::new();
        let mut pooled_mut = pool.insert_mut("hello".to_string());

        let len = char_count(&pooled_mut);
        assert_eq!(len, 5);

        // Test with a simple mutation.
        pooled_mut.push('!');
        assert_eq!(&*pooled_mut, "hello!");

        pool.remove_mut(pooled_mut);
    }

    #[test]
    fn pinning_functionality() {
        let mut pool = RawBlindPool::new();
        let mut pooled_mut = pool.insert_mut("Pin test".to_string());

        // Test as_pin() method
        let pinned_ref = pooled_mut.as_pin();
        assert_eq!(&**pinned_ref, "Pin test");

        // Test as_pin_mut() method
        let pinned_mut = pooled_mut.as_pin_mut();
        pinned_mut.get_mut().push_str(" - modified");

        // Verify the mutation worked
        assert_eq!(&*pooled_mut, "Pin test - modified");

        pool.remove_mut(pooled_mut);
    }

    #[test]
    fn into_shared_conversion() {
        let mut pool = RawBlindPool::new();
        let mut pooled_mut = pool.insert_mut("Test".to_string());

        // Modify while we have exclusive access
        pooled_mut.push_str(" String");
        assert_eq!(&*pooled_mut, "Test String");

        // Convert to shared handle
        let shared = pooled_mut.into_shared();
        assert_eq!(&*shared, "Test String");
        assert_eq!(shared.len(), 11);

        // SAFETY: This pooled handle was just created and has never been used for removal before.
        unsafe {
            pool.remove(&shared);
        }
    }

    #[test]
    fn into_shared_preserves_value() {
        let mut pool = RawBlindPool::new();
        let mut pooled_mut = pool.insert_mut(vec![1, 2, 3]);

        // Modify the vector
        pooled_mut.push(4);
        pooled_mut.push(5);

        let shared = pooled_mut.into_shared();
        assert_eq!(&*shared, &[1, 2, 3, 4, 5]);

        // SAFETY: This pooled handle was just created and has never been used for removal before.
        unsafe {
            pool.remove(&shared);
        }
    }

    #[test]
    fn works_with_different_types() {
        let mut pool = RawBlindPool::new();

        // Test with primitive types
        let mut int_handle = pool.insert_mut(42_i32);
        let mut float_handle = pool.insert_mut(2.71_f64);

        // Modify values
        *int_handle = 84;
        *float_handle = 5.42;

        assert_eq!(*int_handle, 84);
        assert!((*float_handle - 5.42).abs() < f64::EPSILON);

        // Test with complex types
        let mut vec_handle = pool.insert_mut(vec![1, 2, 3]);
        let mut string_handle = pool.insert_mut("Test".to_string());

        vec_handle.push(4);
        string_handle.push_str("ing");

        assert_eq!(*vec_handle, [1, 2, 3, 4]);
        assert_eq!(*string_handle, "Testing");

        // Cleanup
        pool.remove_mut(int_handle);
        pool.remove_mut(float_handle);
        pool.remove_mut(vec_handle);
        pool.remove_mut(string_handle);
    }

    #[test]
    fn debug_impl() {
        let mut pool = RawBlindPool::new();
        let pooled_mut = pool.insert_mut(42_u32);

        let debug_str = format!("{pooled_mut:?}");
        assert!(debug_str.contains("RawPooledMut"));

        pool.remove_mut(pooled_mut);
    }

    #[test]
    fn exclusive_ownership() {
        let mut pool = RawBlindPool::new();
        let pooled_mut = pool.insert_mut("Exclusive".to_string());

        // RawPooledMut should not be Copy or Clone - this ensures exclusive ownership
        // This is a compile-time test - these lines would fail to compile:
        // let _copied = pooled_mut;  // This would move, not copy
        // let _cloned = pooled_mut.clone();  // This should not exist

        // Verify we can still use it after moving around
        let moved_handle = pooled_mut;
        assert_eq!(&*moved_handle, "Exclusive");

        pool.remove_mut(moved_handle);
    }

    #[test]
    fn safe_removal_consumes_handle() {
        let mut pool = RawBlindPool::new();
        let mut pooled_mut = pool.insert_mut("Safe removal".to_string());

        // Modify the value
        pooled_mut.push_str(" test");

        // Safe removal that consumes the handle
        pool.remove_mut(pooled_mut);

        // The handle is consumed and cannot be used again
        // This would fail to compile: assert_eq!(&*pooled_mut, "something");

        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn safe_removal_with_return() {
        let mut pool = RawBlindPool::new();
        let mut pooled_mut = pool.insert_mut("Return test".to_string());

        // Modify the value
        pooled_mut.push('!');

        // Safe removal that returns the value
        let extracted = pool.remove_unpin_mut(pooled_mut);

        assert_eq!(extracted, "Return test!");
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn mutable_access_through_pin() {
        let mut pool = RawBlindPool::new();
        let mut pooled_mut = pool.insert_mut(vec![1, 2, 3]);

        // Get pinned mutable access
        let pinned_mut = pooled_mut.as_pin_mut();

        // Modify through the pin
        pinned_mut.get_mut().push(4);

        // Verify modification
        assert_eq!(*pooled_mut, [1, 2, 3, 4]);

        pool.remove_mut(pooled_mut);
    }

    #[cfg(test)]
    mod static_assertions {
        use std::cell::RefCell;
        use std::marker::PhantomPinned;
        use std::rc::Rc;

        use static_assertions::{assert_impl_all, assert_not_impl_any};

        use super::RawPooledMut;

        #[test]
        fn thread_safety_assertions() {
            // RawPooledMut<T> should be Send+Sync if T is Sync (exclusive access but shareable handle)
            assert_impl_all!(RawPooledMut<u32>: Send, Sync); // u32 is Sync
            assert_impl_all!(RawPooledMut<String>: Send, Sync); // String is Sync
            assert_impl_all!(RawPooledMut<Vec<u8>>: Send, Sync); // Vec<u8> is Sync

            // RawPooledMut<T> should be single-threaded when T is not Send or Sync
            assert_not_impl_any!(RawPooledMut<Rc<u32>>: Send, Sync); // Rc is not Send

            use std::cell::RefCell;
            assert_not_impl_any!(RawPooledMut<RefCell<u32>>: Send, Sync); // RefCell is not Sync

            // Type-erased handles
            assert_impl_all!(RawPooledMut<()>: Send, Sync);
        }

        #[test]
        fn pin_assertions() {
            // RawPooledMut<T> should always be Unpin regardless of T
            assert_impl_all!(RawPooledMut<u32>: Unpin);
            assert_impl_all!(RawPooledMut<String>: Unpin);
            assert_impl_all!(RawPooledMut<Vec<u8>>: Unpin);
            assert_impl_all!(RawPooledMut<Rc<u32>>: Unpin);
            assert_impl_all!(RawPooledMut<RefCell<u32>>: Unpin);
            assert_impl_all!(RawPooledMut<()>: Unpin);

            // Even with non-Unpin types, RawPooledMut should still be Unpin
            assert_impl_all!(RawPooledMut<PhantomPinned>: Unpin);
        }

        #[test]
        fn exclusive_ownership_assertions() {
            // RawPooledMut should NOT be Copy or Clone (exclusive ownership)
            assert_not_impl_any!(RawPooledMut<u32>: Copy, Clone);
            assert_not_impl_any!(RawPooledMut<String>: Copy, Clone);
            assert_not_impl_any!(RawPooledMut<Vec<u8>>: Copy, Clone);
            assert_not_impl_any!(RawPooledMut<()>: Copy, Clone);
        }
    }
}
