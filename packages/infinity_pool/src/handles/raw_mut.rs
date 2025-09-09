use std::fmt;
use std::pin::Pin;
use std::ptr::NonNull;

use crate::{RawPooled, SlabHandle};

/// A unique handle to an object in an object pool.
#[doc = include_str!("../../doc/snippets/raw_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/unique_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/unique_raw_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/nonlocal_handle_thread_safety.md")]
pub struct RawPooledMut<T>
where
    // We support casting to trait objects, hence `?Sized`.
    T: ?Sized,
{
    /// Index of the slab in the pool. Slabs are guaranteed to stay at the same index unless
    /// the pool is shrunk (which can only happen when the affected slabs are empty, in which
    /// case all existing handles are already invalidated).
    slab_index: usize,

    /// Handle to the object in the slab. This grants us access to the object's pointer
    /// and allows us to operate on the object (e.g. to remove it or create a reference).
    slab_handle: SlabHandle<T>,
}

impl<T: ?Sized> RawPooledMut<T> {
    #[must_use]
    pub(crate) fn new(slab_index: usize, slab_handle: SlabHandle<T>) -> Self {
        Self {
            slab_index,
            slab_handle,
        }
    }

    #[doc = include_str!("../../doc/snippets/handle_ptr.md")]
    #[must_use]
    #[inline]
    pub fn ptr(&self) -> NonNull<T> {
        self.slab_handle.ptr()
    }

    /// Get the slab index for this handle.
    #[must_use]
    #[inline]
    #[allow(dead_code, reason = "Used by pool_raw.rs")]
    pub(crate) fn slab_index(&self) -> usize {
        self.slab_index
    }

    /// Get the slab handle for this handle.
    #[must_use]
    #[inline]
    #[allow(dead_code, reason = "Used by pool_raw.rs")]
    pub(crate) fn slab_handle(&self) -> SlabHandle<T> {
        self.slab_handle
    }

    #[doc = include_str!("../../doc/snippets/handle_erase.md")]
    #[must_use]
    #[inline]
    pub fn erase(self) -> RawPooledMut<()> {
        RawPooledMut {
            slab_index: self.slab_index,
            slab_handle: self.slab_handle.erase(),
        }
    }

    #[doc = include_str!("../../doc/snippets/handle_into_shared.md")]
    #[must_use]
    #[inline]
    pub fn into_shared(self) -> RawPooled<T> {
        // Detect type erasure by checking if T is the unit type
        check_for_type_erasure::<T>();

        RawPooled::new(self.slab_index, self.slab_handle)
    }

    /// Internal method to convert to shared handle bypassing type erasure check.
    /// This is used by wrapper types that manage their own type erasure tracking.
    pub(crate) fn into_shared_unchecked(self) -> RawPooled<T> {
        RawPooled::new(self.slab_index, self.slab_handle)
    }

    #[doc = include_str!("../../doc/snippets/raw_as_pin.md")]
    #[must_use]
    #[inline]
    pub unsafe fn as_pin(&self) -> Pin<&T> {
        // SAFETY: Forwarding safety guarantees from the caller.
        let as_ref = unsafe { self.as_ref() };

        // SAFETY: Pooled items are always pinned.
        unsafe { Pin::new_unchecked(as_ref) }
    }

    #[doc = include_str!("../../doc/snippets/raw_as_pin_mut.md")]
    #[must_use]
    #[inline]
    pub unsafe fn as_pin_mut(&mut self) -> Pin<&mut T> {
        // SAFETY: This is a unique handle, so we guarantee borrow safety
        // of the target object by borrowing the handle itself. Pointer validity
        // requires pool to be alive, which is a safety requirement of this function.
        let as_mut = unsafe { self.ptr().as_mut() };

        // SAFETY: Pooled items are always pinned.
        unsafe { Pin::new_unchecked(as_mut) }
    }

    #[doc = include_str!("../../doc/snippets/raw_as_ref.md")]
    #[must_use]
    #[inline]
    pub unsafe fn as_ref(&self) -> &T {
        // SAFETY: This is a unique handle, so we guarantee borrow safety
        // of the target object by borrowing the handle itself. Pointer validity
        // requires pool to be alive, which is a safety requirement of this function.
        unsafe { self.ptr().as_ref() }
    }

    /// Casts this handle to reference the target as a trait object.
    ///
    /// This method is only intended for use by the [`define_pooled_dyn_cast!`] macro
    /// for type-safe casting operations.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the provided closure's input and output references
    /// point to the same object.
    ///
    /// The caller must guarantee that the pool will remain alive for the duration the returned
    /// reference is used.
    #[doc(hidden)]
    #[must_use]
    #[inline]
    pub unsafe fn __private_cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> RawPooledMut<U>
    where
        F: FnOnce(&mut T) -> &mut U,
    {
        // SAFETY: Forwarding callback safety guarantees from the caller.
        // We are an exclusive handle, so we always have the right to create
        // exclusive references to the target of the handle, satisfying that requirement.
        let new_handle = unsafe { self.slab_handle.cast_with_mut(cast_fn) };

        RawPooledMut {
            slab_index: self.slab_index,
            slab_handle: new_handle,
        }
    }
}

impl<T: ?Sized + Unpin> RawPooledMut<T> {
    #[doc = include_str!("../../doc/snippets/raw_as_mut.md")]
    #[must_use]
    #[inline]
    pub unsafe fn as_mut(&mut self) -> &mut T {
        // SAFETY: This is a unique handle, so we guarantee borrow safety
        // of the target object by borrowing the handle itself. Pointer validity
        // requires pool to be alive, which is a safety requirement of this function.
        unsafe { self.ptr().as_mut() }
    }
}

impl<T: ?Sized> fmt::Debug for RawPooledMut<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawPooledMut")
            .field("slab_index", &self.slab_index)
            .field("slab_handle", &self.slab_handle)
            .finish()
    }
}

// SAFETY: RawPooledMut<T> is a unique handle that grants exclusive access to T. When T is Send,
// the exclusive handle can be safely transferred between threads because there are no concurrent
// accesses to T - the handle provides the only way to access T and ensures exclusive ownership.
// The underlying SlabHandle is just a pointer and index which are safe to transfer between threads
// as long as T itself can be moved (T: Send).
unsafe impl<T: ?Sized + Send> Send for RawPooledMut<T> {}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::thread;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use crate::RawPinnedPool;

    use super::*;

    // u32 is Sync, so RawPooledMut<u32> should be Send (but not Sync).
    assert_impl_all!(RawPooledMut<u32>: Send);
    assert_not_impl_any!(RawPooledMut<u32>: Sync);

    // Cell is Send but not Sync, so RawPooledMut<Cell> should now be Send (but not Sync).
    assert_impl_all!(RawPooledMut<Cell<u32>>: Send);
    assert_not_impl_any!(RawPooledMut<Cell<u32>>: Sync);

    // This is a unique handle, it cannot be copyable.
    assert_not_impl_any!(RawPooledMut<u32>: Copy);

    // Test type that is Send but not Sync.
    struct SendNotSync {
        data: Cell<i32>,
    }

    // SAFETY: Cell<T> is Send if T is Send.
    unsafe impl Send for SendNotSync {}

    #[test]
    fn unique_raw_handle_works_with_send_not_sync() {
        let mut pool = RawPinnedPool::<SendNotSync>::new();
        let handle = pool.insert(SendNotSync {
            data: Cell::new(55),
        });

        // Unique raw handles should be Send even when T is !Sync but Send.
        // Note: We need to use unsafe access methods for raw handles.
        let result = thread::spawn(move || {
            // SAFETY: The handle is unique and we own it, so we can create a reference.
            // The pool remains alive for the duration of the test.
            let value = unsafe { handle.as_ref() };
            value.data.get()
        })
        .join()
        .unwrap();

        assert_eq!(result, 55);
    }
}

// Helper function to detect type erasure using type_name
#[inline]
fn check_for_type_erasure<T: ?Sized>() {
    // Use type_name to detect if T is the unit type
    use std::any::type_name;
    if type_name::<T>() == "()" {
        panic!(
            "Cannot create shared handle from type-erased handle. Type-erase after creating shared handle instead."
        );
    }
}
