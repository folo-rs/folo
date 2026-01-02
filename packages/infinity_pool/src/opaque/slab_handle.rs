use std::any::type_name;
use std::fmt;
use std::ptr::{self, NonNull};

/// A shared handle to an object stored in a [`Slab`].
///
/// This can be used to obtain a typed pointer to the inserted item, as well as to remove
/// the item from the slab when no longer needed.
///
/// The handle enforces no ownership semantics - consider it merely a fat pointer. It can be
/// freely cloned and copied. The only way to access the contents is unsafe access through
/// the pointer obtained via `ptr()`.
///
/// # Thread safety
///
/// The handle provides access to the underlying object, so its thread-safety characteristics
/// are determined by the type of the object it points to.
///
/// If `T` is `Send` then the handle is also `Send`. This enables the handle to be used on the
/// other thread to remove the object from the slab (assuming the entire slab is thread-mobile,
/// which it may not be - depends on the type of collection it is part of and what it contains).
/// This is because the slab requires an owned slab handle to remove an object, so a !Send slab
/// handle prevents the object from being removed on a thread it was not inserted on.
///
/// This handle itself is safe to access from multiple threads, as it is immutable. Of course, it
/// provides no direct access to the object (only a pointer) and the object of type `T` might not
/// be safe to access across threads. However, any access of the object `T` is outside the
/// jurisdiction of this handle type.
pub(crate) struct SlabHandle<T: ?Sized> {
    /// Index in the slab at which this item is stored.
    ///
    /// This is used for removal of the object.
    ///
    /// Note that slab indexes may be reused after an object is removed. It is the responsibility
    /// of the caller to not use a handler after the object has been removed (this is generally
    /// going to be a safety requirement of any method that returns `SlabHandle`).
    index: usize,

    /// The object this handle points to in the slab.
    ///
    /// Note that slab slots may be reused after an object is removed. It is the responsibility
    /// of the caller to not use a handler after the object has been removed (this is generally
    /// going to be a safety requirement of any method that returns `SlabHandle`).
    ptr: NonNull<T>,
}

impl<T: ?Sized> SlabHandle<T> {
    #[must_use]
    pub(crate) fn new(index: usize, ptr: NonNull<T>) -> Self {
        Self { index, ptr }
    }

    /// Get the index of the object in the slab.
    ///
    /// This is used by the slab itself to identify the slot to be freed upon removal.
    #[must_use]
    pub(crate) fn index(&self) -> usize {
        self.index
    }

    /// Casts this handle to reference the target as a trait object.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the target object is in a state where it is
    /// valid to create a shared reference to it (i.e. no concurrent `&mut` exclusive
    /// references exist). Any created temporary references will have been released
    /// by the time the method returns.
    ///
    /// The caller must guarantee that the provided closure's input and output references
    /// point to the same object.
    #[must_use]
    pub(crate) unsafe fn cast_with<U: ?Sized, F>(self, cast_fn: F) -> SlabHandle<U>
    where
        F: FnOnce(&T) -> &U,
    {
        // Get a reference to perform the cast and obtain the trait object pointer.
        // SAFETY: Forwarding safety requirements to the caller.
        let value_ref = unsafe { self.ptr.as_ref() };

        // Use the provided function to perform the cast - this ensures type safety.
        let new_ref: &U = cast_fn(value_ref);

        // Now we need a pointer to stuff into a new SlabHandle.
        let new_ptr = NonNull::from(new_ref);

        // Create a new slab handle with the trait object type.
        SlabHandle {
            index: self.index,
            ptr: new_ptr,
        }
    }

    /// Casts this handle to reference the target as a trait object with exclusive access.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the target object is in a state where it is
    /// valid to create an exclusive reference to it (i.e. no concurrent references exist).
    /// Any created temporary references will have been released
    /// by the time the method returns.
    ///
    /// The caller must guarantee that the provided closure's input and output references
    /// point to the same object.
    #[must_use]
    pub(crate) unsafe fn cast_with_mut<U: ?Sized, F>(mut self, cast_fn: F) -> SlabHandle<U>
    where
        F: FnOnce(&mut T) -> &mut U,
    {
        // Get a reference to perform the cast and obtain the trait object pointer.
        // SAFETY: Forwarding safety requirements to the caller.
        let value_ref = unsafe { self.ptr.as_mut() };

        // Use the provided function to perform the cast - this ensures type safety.
        let new_ref: &mut U = cast_fn(value_ref);

        // Now we need a pointer to stuff into a new SlabHandle.
        let new_ptr = NonNull::from(new_ref);

        // Create a new slab handle with the trait object type.
        SlabHandle {
            index: self.index,
            ptr: new_ptr,
        }
    }

    /// Erase the type information from this handle.
    ///
    /// This is used internally by the pool cleanup system to allow uniform handling
    /// of handles with different types, particularly for shared handle cleanup where
    /// different trait object casts would otherwise create incompatible remover types.
    ///
    /// # Safety
    ///
    /// This operation may upgrade the handle from `!Send` to `Send`. The caller is
    /// responsible for ensuring that the resulting handle does not allow the object `T`
    /// to be accessed in a way that would violate its original thread-safety guarantees.
    ///
    /// Most importantly, this means if the handle is itself a reference-counting handle,
    /// it must only be possible to erase it if `T: Send` because otherwise a type-erased
    /// handle may end up trying to destroy a `T: !Send` on a different thread (which is invalid).
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    pub(crate) unsafe fn erase(self) -> SlabHandle<()> {
        SlabHandle {
            index: self.index,
            ptr: self.ptr.cast::<()>(),
        }
    }

    /// Get a raw pointer to the object in the slab.
    ///
    /// It is the responsibility of the caller to ensure that the pointer is not used
    /// after the object has been removed from the slab.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    pub(crate) fn ptr(&self) -> NonNull<T> {
        self.ptr
    }
}

impl<T: ?Sized> Clone for SlabHandle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> Copy for SlabHandle<T> {}

impl<T: ?Sized> PartialEq for SlabHandle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index && ptr::addr_eq(self.ptr.as_ptr(), other.ptr.as_ptr())
    }
}

impl<T: ?Sized> Eq for SlabHandle<T> {}

// SAFETY: See type-level documentation.
unsafe impl<T> Send for SlabHandle<T> where T: ?Sized + Send {}

// SAFETY: See type-level documentation.
unsafe impl<T> Sync for SlabHandle<T> where T: ?Sized {}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: ?Sized> fmt::Debug for SlabHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("index", &self.index)
            .field("ptr", &self.ptr)
            .finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::fmt::Display;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::{NotSendNotSync, NotSendSync, SendAndSync, SendNotSync};

    assert_impl_all!(SlabHandle<SendAndSync>: Send, Sync);
    assert_impl_all!(SlabHandle<SendNotSync>: Send, Sync);
    assert_impl_all!(SlabHandle<NotSendNotSync>: Sync);
    assert_impl_all!(SlabHandle<NotSendSync>: Sync);

    assert_not_impl_any!(SlabHandle<NotSendNotSync>: Send);
    assert_not_impl_any!(SlabHandle<NotSendSync>: Send);

    // We expect it to work as a value type.
    assert_impl_all!(SlabHandle<SendAndSync>: Copy);
    assert_not_impl_any!(SlabHandle<SendAndSync>: Drop);

    #[test]
    fn same_index_different_ptr_is_not_equal() {
        let mut x = 42_u32;
        let mut y = 42_u32;
        let handle1 = SlabHandle::new(0, NonNull::from(&mut x));
        let handle2 = SlabHandle::new(0, NonNull::from(&mut y));
        assert_ne!(handle1, handle2);
    }

    #[test]
    fn same_index_same_ptr_is_equal() {
        let mut x = 42_u32;
        let ptr = NonNull::from(&mut x);
        let handle1 = SlabHandle::new(0, ptr);
        let handle2 = SlabHandle::new(0, ptr);
        assert_eq!(handle1, handle2);
    }

    #[test]
    fn different_index_same_ptr_is_not_equal() {
        let mut x = 42_u32;
        let ptr = NonNull::from(&mut x);
        let handle1 = SlabHandle::new(0, ptr);
        let handle2 = SlabHandle::new(1, ptr);
        assert_ne!(handle1, handle2);
    }

    #[test]
    fn clone_copy_are_equal() {
        let mut x = 42_u32;
        let handle = SlabHandle::new(5, NonNull::from(&mut x));
        let cloned = Clone::clone(&handle);
        let copied = handle;
        assert_eq!(handle, cloned);
        assert_eq!(handle, copied);
        assert_eq!(cloned, copied);
    }

    #[test]
    fn cast_with_to_display_trait() {
        let mut value = 42_usize;
        let handle: SlabHandle<usize> = SlabHandle::new(10, NonNull::from(&mut value));

        // Cast from SlabHandle<usize> to SlabHandle<dyn Display>
        // SAFETY: We control the lifetime and the value is valid for the duration of the test
        let display_handle: SlabHandle<dyn Display> = unsafe {
            handle.cast_with(|x: &usize| {
                let display_ref: &dyn Display = x;
                display_ref
            })
        };

        // Verify the index is preserved
        assert_eq!(display_handle.index(), 10);

        // Verify we can use the trait object
        // SAFETY: The handle points to valid data for the test duration
        let display_str = unsafe { display_handle.ptr().as_ref().to_string() };
        assert_eq!(display_str, "42");
    }

    #[test]
    fn cast_with_mut_to_display_trait() {
        let mut value = 123_u64;
        let handle: SlabHandle<u64> = SlabHandle::new(7, NonNull::from(&mut value));

        // Cast from SlabHandle<u64> to SlabHandle<dyn Display>
        // SAFETY: We control the lifetime and the value is valid for the duration of the test
        let display_handle: SlabHandle<dyn Display> = unsafe {
            handle.cast_with_mut(|x: &mut u64| {
                let display_ref: &mut dyn Display = x;
                display_ref
            })
        };

        // Verify the index is preserved
        assert_eq!(display_handle.index(), 7);

        // Verify we can use the trait object
        // SAFETY: The handle points to valid data for the test duration
        let display_str = unsafe { display_handle.ptr().as_ref().to_string() };
        assert_eq!(display_str, "123");
    }

    // Define a simple trait for testing mutation through trait objects
    trait Incrementable {
        fn increment(&mut self);
        fn get_value(&self) -> u32;
    }

    impl Incrementable for u32 {
        fn increment(&mut self) {
            *self = self.wrapping_add(1);
        }

        fn get_value(&self) -> u32 {
            *self
        }
    }

    #[test]
    fn cast_with_mut_allows_mutation() {
        let mut value = 100_u32;
        let handle: SlabHandle<u32> = SlabHandle::new(15, NonNull::from(&mut value));

        // Cast to mutable trait object
        // SAFETY: We control the lifetime and the value is valid for the duration of the test
        let mut_handle: SlabHandle<dyn Incrementable> = unsafe {
            handle.cast_with_mut(|x: &mut u32| {
                let trait_ref: &mut dyn Incrementable = x;
                trait_ref
            })
        };

        // Verify index is preserved
        assert_eq!(mut_handle.index(), 15);

        // Verify initial value through trait object
        // SAFETY: The handle points to valid data for the test duration
        assert_eq!(unsafe { mut_handle.ptr().as_ref().get_value() }, 100);

        // Mutate through the trait object
        // SAFETY: We have exclusive access to the data for the test duration
        unsafe {
            mut_handle.ptr().as_mut().increment();
        }

        // Verify the mutation worked through trait object
        // SAFETY: The handle points to valid data for the test duration
        assert_eq!(unsafe { mut_handle.ptr().as_ref().get_value() }, 101);
    }
}
