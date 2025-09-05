use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;

use crate::{RawPooledMut, SlabHandle};

/// A shared handle to an object in an object pool.
///
/// The handle can be used to access the pooled object, as well as to remove
/// it from the pool when no longer needed.
///
/// This is a raw handle that requires manual lifetime management of the pooled objects.
/// You must call `remove()` on the pool to drop the object this handle references.
/// If the handle is dropped without being passed to `remove()`, the object is only removed
/// from the pool when the pool itself is dropped. 
///
/// This is a shared handle that only grants shared access to the object. No exclusive
/// references can be created through this handle.
///
/// # Thread safety
///
/// The handle provides access to the underlying object, so its thread-safety characteristics
/// are determined by the type of the object it points to.
///
/// If the underlying object is `Sync`, the handle is thread-mobile (`Send`). Otherwise, the
/// handle is single-threaded (neither `Send` nor `Sync`).
pub struct RawPooled<T: ?Sized> {
    /// Index of the slab in the pool. Slabs are guaranteed to stay at the same index unless
    /// the pool is shrunk (which can only happen when the affected slabs are empty, in which
    /// case all existing handles are already invalidated).
    slab_index: usize,

    /// Handle to the object in the slab. This grants us access to the object's pointer
    /// and allows us to operate on the object (e.g. to remove it).
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

    /// Get a pointer to the object in the pool.
    #[must_use]
    pub fn ptr(&self) -> NonNull<T> {
        self.slab_handle.ptr()
    }

    /// Get the slab handle for this pool handle.
    ///
    /// This is used by the pool itself to perform operations on the object in the slab.
    #[must_use]
    pub(crate) fn slab_handle(&self) -> SlabHandle<T> {
        self.slab_handle
    }

    /// Erases the type of the object the pool handle points to.
    ///
    /// The returned handle remains functional for most purposes, just without type information.
    /// A type-erased handle cannot be used to remove the object from the pool and return it to
    /// the caller, as there is no more knowledge of the type to be returned.
    #[must_use]
    pub fn erase(self) -> RawPooled<()> {
        RawPooled {
            slab_index: self.slab_index,
            slab_handle: self.slab_handle.erase(),
        }
    }

    /// Borrows the target object as a pinned shared reference.
    ///
    /// All pooled objects are guaranteed to be pinned for their entire lifetime.
    #[must_use]
    pub fn as_pin(&self) -> Pin<&T> {
        // SAFETY: Pooled items are always pinned.
        unsafe { Pin::new_unchecked(self) }
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
    #[doc(hidden)]
    #[must_use]
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

impl<T: ?Sized> Deref for RawPooled<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: This is a shared handle - the only references
        // that can ever exist are shared references.
        unsafe { self.ptr().as_ref() }
    }
}

impl<T: ?Sized> Borrow<T> for RawPooled<T> {
    fn borrow(&self) -> &T {
        self
    }
}

impl<T: ?Sized> AsRef<T> for RawPooled<T> {
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T: ?Sized> From<RawPooledMut<T>> for RawPooled<T> {
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
