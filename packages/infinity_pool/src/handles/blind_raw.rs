use std::alloc::Layout;
use std::fmt;
use std::pin::Pin;
use std::ptr::NonNull;

use crate::{RawBlindPooledMut, RawPooled};

/// A shared handle to an object in a [`RawBlindPool`][1].
#[doc = include_str!("../../doc/snippets/raw_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/shared_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/shared_raw_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/raw_handle_thread_safety.md")]
/// [1]: crate::RawBlindPool
pub struct RawBlindPooled<T>
where
    // We support casting to trait objects, hence `?Sized`.
    T: ?Sized,
{
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

    #[doc = include_str!("../../doc/snippets/handle_ptr.md")]
    #[must_use]
    pub fn ptr(&self) -> NonNull<T> {
        self.inner.ptr()
    }

    #[doc = include_str!("../../doc/snippets/handle_erase.md")]
    #[must_use]
    pub fn erase(self) -> RawBlindPooled<()> {
        RawBlindPooled {
            layout: self.layout,
            inner: self.inner.erase(),
        }
    }

    #[doc = include_str!("../../doc/snippets/raw_as_pin.md")]
    #[must_use]
    pub unsafe fn as_pin(&self) -> Pin<&T> {
        // SAFETY: Forwarding safety guarantees from the caller.
        let as_ref = unsafe { self.as_ref() };

        // SAFETY: Pooled items are always pinned.
        unsafe { Pin::new_unchecked(as_ref) }
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
    #[doc(hidden)]
    #[must_use]
    pub unsafe fn __private_cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> RawBlindPooled<U>
    where
        F: FnOnce(&T) -> &U,
    {
        // SAFETY: Forwarding callback safety guarantees from the caller.
        // We are a shared handle, so we always have the right to create
        // shared references to the target of the handle, satisfying that requirement.
        let new_inner = unsafe { self.inner.__private_cast_dyn_with_fn(cast_fn) };

        RawBlindPooled {
            layout: self.layout,
            inner: new_inner,
        }
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
