use std::fmt;
use std::ptr::NonNull;

use crate::SlabHandle;

/// A shared handle to an object in a [`Pool`].
///
/// Public API types will hide this behind smarter handle mechanisms that may know the type of
/// the object and may have lifetime management semantics.
///
/// This can be used to obtain a typed pointer to the inserted item, as well as to remove
/// the item from the pool when no longer needed.
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
/// If the underlying object is `Sync`, the handle is thread-mobile (`Send`). Otherwise, the
/// handle is single-threaded (neither `Send` nor `Sync`).
pub(crate) struct PoolHandle<T: ?Sized> {
    /// Index of the slab in the pool. Slabs are guaranteed to say at the same index unless
    /// the pool is shrunk (which can only happen when the affected slabs are empty, in which
    /// case all existing handles are already invalidated).
    slab_index: usize,

    /// Handle to the object in the slab. This grants us access to the object's pointer
    /// and allows us to operate on the object (e.g. to remove it).
    slab_handle: SlabHandle<T>,
}

impl<T: ?Sized> PoolHandle<T> {
    pub(crate) fn new(slab_index: usize, slab_handle: SlabHandle<T>) -> Self {
        Self {
            slab_index,
            slab_handle,
        }
    }

    /// Get the index of the slab in the pool.
    ///
    /// This is used by the pool itself to identify the slab in which the object resides.
    pub(crate) fn slab_index(&self) -> usize {
        self.slab_index
    }

    /// Get a raw pointer to the object in the pool.
    ///
    /// It is the responsibility of the caller to ensure that the pointer is not used
    /// after the object has been removed from the pool.
    pub(crate) fn ptr(&self) -> NonNull<T> {
        self.slab_handle.ptr()
    }

    /// Get the slab handle for this pool handle.
    ///
    /// This is used by the pool itself to perform operations on the object in the slab.
    pub(crate) fn slab_handle(&self) -> SlabHandle<T> {
        self.slab_handle
    }

    /// Erases the type of the object the pool handle points to.
    ///
    /// The returned handle remains functional for most purposes, just without type information.
    /// A type-erased handle cannot be used to remove the object from the pool and return it to
    /// the caller, as there is no more knowledge of the type to be returned.
    pub(crate) fn erase(self) -> PoolHandle<()> {
        PoolHandle {
            slab_index: self.slab_index,
            slab_handle: self.slab_handle.erase(),
        }
    }
}

impl<T: ?Sized> Clone for PoolHandle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> Copy for PoolHandle<T> {}

impl<T: ?Sized> PartialEq for PoolHandle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.slab_index == other.slab_index && self.slab_handle == other.slab_handle
    }
}

impl<T: ?Sized> Eq for PoolHandle<T> {}

impl<T: ?Sized> fmt::Debug for PoolHandle<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PoolHandle")
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

    // u32 is Sync, so PoolHandle<u32> should be Send (but not Sync).
    assert_impl_all!(PoolHandle<u32>: Send);
    assert_not_impl_any!(PoolHandle<u32>: Sync);

    // Cell is Send but not Sync, so PoolHandle<Cell> should be neither Send nor Sync.
    assert_not_impl_any!(PoolHandle<Cell<u32>>: Send, Sync);
}
