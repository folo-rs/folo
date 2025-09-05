use std::fmt;
use std::pin::Pin;
use std::ptr::NonNull;

use crate::{RawPooled, SlabHandle};

/// A unique handle to an object in an object pool.
#[doc = include_str!("../../doc/snippets/raw_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/unique_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/unique_raw_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/raw_handle_thread_safety.md")]
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
    pub fn ptr(&self) -> NonNull<T> {
        self.slab_handle.ptr()
    }

    #[doc = include_str!("../../doc/snippets/handle_erase.md")]
    #[must_use]
    pub fn erase(self) -> RawPooledMut<()> {
        RawPooledMut {
            slab_index: self.slab_index,
            slab_handle: self.slab_handle.erase(),
        }
    }

    /// Transforms the unique handle into a shared handle that can be cloned and copied freely.
    ///
    /// A shared handle does not support the creation of exclusive references to the target object
    /// and requires the caller to guarantee that no further access is attempted through any handle
    /// after removing the object from the pool.
    #[must_use]
    pub fn into_shared(self) -> RawPooled<T> {
        RawPooled::new(self.slab_index, self.slab_handle)
    }

    #[doc = include_str!("../../doc/snippets/raw_as_pin.md")]
    #[must_use]
    pub unsafe fn as_pin(&self) -> Pin<&T> {
        // SAFETY: Forwarding safety guarantees from the caller.
        let as_ref = unsafe { self.as_ref() };

        // SAFETY: Pooled items are always pinned.
        unsafe { Pin::new_unchecked(as_ref) }
    }

    #[doc = include_str!("../../doc/snippets/raw_as_pin_mut.md")]
    #[must_use]
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    // u32 is Sync, so RawPooledMut<u32> should be Send (but not Sync).
    assert_impl_all!(RawPooledMut<u32>: Send);
    assert_not_impl_any!(RawPooledMut<u32>: Sync);

    // Cell is Send but not Sync, so RawPooledMut<Cell> should be neither Send nor Sync.
    assert_not_impl_any!(RawPooledMut<Cell<u32>>: Send, Sync);
}
