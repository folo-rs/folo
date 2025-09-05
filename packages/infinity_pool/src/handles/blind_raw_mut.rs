use std::alloc::Layout;
use std::borrow::{Borrow, BorrowMut};
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::ptr::NonNull;

use crate::{RawBlindPooled, RawPooledMut};

/// A unique handle to an object in a [`RawBlindPool`][1].
///
/// The handle can be used to access the pooled object, as well as to remove
/// it from the pool when no longer needed.
///
/// This is a raw handle that requires manual lifetime management of the pooled objects.
/// You must call `remove_mut()` on the pool to drop the object this handle references.
/// If the handle is dropped without being passed to `remove_mut()`, the object is only removed
/// from the pool when the pool itself is dropped.
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
///
/// [1]: crate::RawBlindPool
pub struct RawBlindPooledMut<T: ?Sized> {
    // We combine the inner RawPooledMut with a layout that
    // acts as the inner pool key for the RawBlindPool.
    layout: Layout,

    inner: RawPooledMut<T>,
}

impl<T: ?Sized> RawBlindPooledMut<T> {
    #[must_use]
    pub(crate) fn new(layout: Layout, inner: RawPooledMut<T>) -> Self {
        Self { layout, inner }
    }

    /// The layout originally used to insert the item
    ///
    /// This might not match `T` any more, as the `T` parameter may have been transformed.
    pub(crate) fn layout(&self) -> Layout {
        self.layout
    }

    /// Becomes the inner handle for the `RawOpaquePool` that holds the object.
    pub(crate) fn into_inner(self) -> RawPooledMut<T> {
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
    pub fn erase(self) -> RawBlindPooledMut<()> {
        RawBlindPooledMut {
            layout: self.layout,
            inner: self.inner.erase(),
        }
    }

    /// Transforms the unique handle into a shared handle that can be cloned and copied freely.
    #[must_use]
    pub fn into_shared(self) -> RawBlindPooled<T> {
        RawBlindPooled::new(self.layout, self.inner.into_shared())
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
    pub unsafe fn __private_cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> RawBlindPooledMut<U>
    where
        F: FnOnce(&mut T) -> &mut U,
    {
        // SAFETY: Forwarding callback safety guarantees from the caller.
        // We are an exclusive handle, so we always have the right to create
        // exclusive references to the target of the handle, satisfying that requirement.
        let new_inner = unsafe { self.inner.__private_cast_dyn_with_fn(cast_fn) };

        RawBlindPooledMut {
            layout: self.layout,
            inner: new_inner,
        }
    }
}

impl<T: ?Sized> fmt::Debug for RawBlindPooledMut<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawBlindPooledMut")
            .field("layout", &self.layout)
            .field("inner", &self.inner)
            .finish()
    }
}

impl<T: ?Sized> Deref for RawBlindPooledMut<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: This is a unique handle, so we guarantee borrow safety
        // of the target object by borrowing the handle itself.
        unsafe { self.ptr().as_ref() }
    }
}

impl<T> DerefMut for RawBlindPooledMut<T>
where
    T: ?Sized + Unpin,
{
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: This is a unique handle, so we guarantee borrow safety
        // of the target object by borrowing the handle itself.
        unsafe { self.ptr().as_mut() }
    }
}

impl<T: ?Sized> Borrow<T> for RawBlindPooledMut<T> {
    fn borrow(&self) -> &T {
        self
    }
}

impl<T> BorrowMut<T> for RawBlindPooledMut<T>
where
    T: ?Sized + Unpin,
{
    fn borrow_mut(&mut self) -> &mut T {
        self
    }
}

impl<T: ?Sized> AsRef<T> for RawBlindPooledMut<T> {
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T> AsMut<T> for RawBlindPooledMut<T>
where
    T: ?Sized + Unpin,
{
    fn as_mut(&mut self) -> &mut T {
        self
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    // u32 is Sync, so RawBlindPooledMut<u32> should be Send (but not Sync).
    assert_impl_all!(RawBlindPooledMut<u32>: Send);
    assert_not_impl_any!(RawBlindPooledMut<u32>: Sync);

    // Cell is Send but not Sync, so RawBlindPooledMut<Cell> should be neither Send nor Sync.
    assert_not_impl_any!(RawBlindPooledMut<Cell<u32>>: Send, Sync);
}
