use std::fmt;
use std::pin::Pin;
use std::ptr::NonNull;

use crate::{RawPooledMut, SlabHandle};

/// A shared handle to an object in an object pool.
#[doc = include_str!("../../doc/snippets/raw_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/shared_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/shared_raw_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/nonlocal_handle_thread_safety.md")]
pub struct RawPooled<T>
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

impl<T: ?Sized> RawPooled<T> {
    #[must_use]
    pub(crate) fn new(slab_index: usize, slab_handle: SlabHandle<T>) -> Self {
        Self {
            slab_index,
            slab_handle,
        }
    }

    /// Get the index of the slab in the pool.
    ///
    /// This is used by the pool itself to identify the slab in which the object resides.
    #[must_use]
    pub(crate) fn slab_index(&self) -> usize {
        self.slab_index
    }

    /// Get the slab handle for this pool handle.
    ///
    /// This is used by the pool itself to perform operations on the object in the slab.
    #[must_use]
    pub(crate) fn slab_handle(&self) -> SlabHandle<T> {
        self.slab_handle
    }

    #[doc = include_str!("../../doc/snippets/handle_ptr.md")]
    #[must_use]
    #[inline]
    pub fn ptr(&self) -> NonNull<T> {
        self.slab_handle.ptr()
    }

    #[doc = include_str!("../../doc/snippets/handle_erase.md")]
    #[must_use]
    #[inline]
    pub fn erase(self) -> RawPooled<()> {
        RawPooled {
            slab_index: self.slab_index,
            slab_handle: self.slab_handle.erase(),
        }
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
    pub unsafe fn __private_cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> RawPooled<U>
    where
        F: FnOnce(&T) -> &U,
    {
        // SAFETY: Forwarding callback safety guarantees from the caller.
        // We are a shared handle, so we always have the right to create
        // shared references to the target of the handle, satisfying that requirement.
        let new_handle = unsafe { self.slab_handle.cast_with(cast_fn) };

        RawPooled {
            slab_index: self.slab_index,
            slab_handle: new_handle,
        }
    }
}

impl<T: ?Sized> Clone for RawPooled<T> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> Copy for RawPooled<T> {}

impl<T: ?Sized> fmt::Debug for RawPooled<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawPooled")
            .field("slab_index", &self.slab_index)
            .field("slab_handle", &self.slab_handle)
            .finish()
    }
}

impl<T: ?Sized> From<RawPooledMut<T>> for RawPooled<T> {
    #[inline]
    fn from(value: RawPooledMut<T>) -> Self {
        value.into_shared()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    // u32 is Sync, so RawPooled<u32> should be Send (but not Sync).
    assert_impl_all!(RawPooled<u32>: Send);
    assert_not_impl_any!(RawPooled<u32>: Sync);

    // Cell is Send but not Sync, so RawPooled<Cell> should be neither Send nor Sync.
    assert_not_impl_any!(RawPooled<Cell<u32>>: Send, Sync);
}
