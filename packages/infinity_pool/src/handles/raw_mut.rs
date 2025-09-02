use std::borrow::{Borrow, BorrowMut};
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::ptr::NonNull;

use crate::{RawPooled, SlabHandle};

/// A unique handle to an object in an object pool.
///
/// The handle can be used to access the pooled object, as well as to remove
/// the item from the pool when no longer needed.
///
/// This is a raw handle that requires manual lifetime management of the pooled objects.
/// You must call `remove_mut()` on the pool to drop the object this handle references.
/// If the handle is dropped without being passed to `remove_mut()`, the object is only removed
/// from the pool when the pool itself is is dropped. Different types of handles may offer
/// automatic removal of the object from the pool when the handle is dropped.
///
/// This is a unique handle, guaranteeing that no other handles to the same object exist. You may
/// create both shared and exclusive references to the object through this handle. The handle may
/// also be converted to a shared handle via `.into_shared()`.
///
/// # Thread safety
///
/// The handle provides access to the underlying object, so its thread-safety characteristics
/// are determined by the type of the object it points to.
///
/// If the underlying object is `Sync`, the handle is thread-mobile (`Send`). Otherwise, the
/// handle is single-threaded (neither `Send` nor `Sync`).
pub struct RawPooledMut<T: ?Sized> {
    /// Index of the slab in the pool. Slabs are guaranteed to say at the same index unless
    /// the pool is shrunk (which can only happen when the affected slabs are empty, in which
    /// case all existing handles are already invalidated).
    slab_index: usize,

    /// Handle to the object in the slab. This grants us access to the object's pointer
    /// and allows us to operate on the object (e.g. to remove it).
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

    /// Get a pointer to the object in the pool.
    #[must_use]
    pub fn ptr(&self) -> NonNull<T> {
        self.slab_handle.ptr()
    }

    /// Erases the type of the object the pool handle points to.
    ///
    /// The returned handle remains functional for most purposes, just without type information.
    /// A type-erased handle cannot be used to remove the object from the pool and return it to
    /// the caller, as there is no more knowledge of the type to be returned.
    #[must_use]
    pub fn erase(self) -> RawPooledMut<()> {
        RawPooledMut {
            slab_index: self.slab_index,
            slab_handle: self.slab_handle.erase(),
        }
    }

    /// Transforms the unique handle into a shared handle that can be cloned and copied freely.
    #[must_use]
    pub fn into_shared(self) -> RawPooled<T> {
        RawPooled::new(self.slab_index, self.slab_handle)
    }

    /// Borrows the target object as a pinned shared reference.
    ///
    /// All pooled objects are guaranteed to be pinned for their entire lifetime.
    #[must_use]
    pub fn as_pin(&self) -> Pin<&T> {
        // SAFETY: Pooled items are always pinned.
        unsafe { Pin::new_unchecked(self) }
    }

    /// Borrows the target object as a pinned exclusive reference.
    ///
    /// All pooled objects are guaranteed to be pinned for their entire lifetime.
    #[must_use]
    pub fn as_pin_mut(&mut self) -> Pin<&mut T> {
        // SAFETY: This is a unique handle, so we guarantee borrow safety
        // of the target object by borrowing the handle itself.
        let as_mut = unsafe { self.ptr().as_mut() };

        // SAFETY: Pooled items are always pinned.
        unsafe { Pin::new_unchecked(as_mut) }
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

impl<T: ?Sized> Deref for RawPooledMut<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: This is a unique handle, so we guarantee borrow safety
        // of the target object by borrowing the handle itself.
        unsafe { self.ptr().as_ref() }
    }
}

impl<T: ?Sized + Unpin> DerefMut for RawPooledMut<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: This is a unique handle, so we guarantee borrow safety
        // of the target object by borrowing the handle itself.
        unsafe { self.ptr().as_mut() }
    }
}

impl<T: ?Sized> Borrow<T> for RawPooledMut<T> {
    fn borrow(&self) -> &T {
        self
    }
}

impl<T: ?Sized + Unpin> BorrowMut<T> for RawPooledMut<T> {
    fn borrow_mut(&mut self) -> &mut T {
        self
    }
}

impl<T: ?Sized> AsRef<T> for RawPooledMut<T> {
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T: ?Sized + Unpin> AsMut<T> for RawPooledMut<T> {
    fn as_mut(&mut self) -> &mut T {
        self
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
