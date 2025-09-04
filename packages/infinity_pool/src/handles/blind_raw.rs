use std::alloc::Layout;
use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;

use crate::{RawBlindPooledMut, RawPooled};

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
pub struct RawBlindPooled<T: ?Sized> {
    // We combine the inner RawPooled with a layout that
    // acts as the inner pool key for the RawBlindPool.
    layout: Layout,

    inner: RawPooled<T>,
}

impl<T: ?Sized> RawBlindPooled<T> {
    #[must_use]
    pub(crate) fn new(layout: Layout, inner: RawPooled<T>) -> Self {
        Self { layout, inner }
    }

    /// The layout originally used to insert the item
    ///
    /// This might not match `T` any more, as the `T` parameter may have been transformed.
    pub(crate) fn layout(&self) -> Layout {
        self.layout
    }

    /// Becomes the inner handle for the `RawOpaquePool` that holds the object.
    pub(crate) fn into_inner(self) -> RawPooled<T> {
        self.inner
    }

    /// Get a pointer to the object in the pool.
    #[must_use]
    pub fn ptr(&self) -> NonNull<T> {
        self.inner.ptr()
    }

    /// Erases the type of the object the pool handle points to.
    ///
    /// The returned handle remains functional for most purposes, just without type information.
    /// A type-erased handle cannot be used to remove the object from the pool and return it to
    /// the caller, as there is no more knowledge of the type to be returned.
    #[must_use]
    pub fn erase(self) -> RawBlindPooled<()> {
        RawBlindPooled {
            layout: self.layout,
            inner: self.inner.erase(),
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
}

impl<T: ?Sized> Clone for RawBlindPooled<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> Copy for RawBlindPooled<T> {}

impl<T: ?Sized> fmt::Debug for RawBlindPooled<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawBlindPooled")
            .field("layout", &self.layout)
            .field("inner", &self.inner)
            .finish()
    }
}

impl<T: ?Sized> Deref for RawBlindPooled<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: This is a shared handle - the only references
        // that can ever exist are shared references.
        unsafe { self.ptr().as_ref() }
    }
}

impl<T: ?Sized> Borrow<T> for RawBlindPooled<T> {
    fn borrow(&self) -> &T {
        self
    }
}

impl<T: ?Sized> AsRef<T> for RawBlindPooled<T> {
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T: ?Sized> From<RawBlindPooledMut<T>> for RawBlindPooled<T> {
    fn from(value: RawBlindPooledMut<T>) -> Self {
        value.into_shared()
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    // u32 is Sync, so RawBlindPooled<u32> should be Send (but not Sync).
    assert_impl_all!(RawBlindPooled<u32>: Send);
    assert_not_impl_any!(RawBlindPooled<u32>: Sync);

    // Cell is Send but not Sync, so RawBlindPooled<Cell> should be neither Send nor Sync.
    assert_not_impl_any!(RawBlindPooled<Cell<u32>>: Send, Sync);
}
