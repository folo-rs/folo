use std::borrow::{Borrow, BorrowMut};
use std::cell::RefCell;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::Rc;
use std::{fmt, mem, ptr};

use crate::{LocalPooled, RawOpaquePool, RawPooledMut};

/// A unique single-threaded reference-counting handle for a pooled object.
#[doc = include_str!("../../doc/snippets/ref_counted_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/unique_handle_implications.md")]
///
/// # Thread safety
///
/// This type is single-threaded.
pub struct LocalPooledMut<T: ?Sized> {
    inner: RawPooledMut<T>,
    pool: Rc<RefCell<RawOpaquePool>>,
}

impl<T: ?Sized> LocalPooledMut<T> {
    #[must_use]
    pub(crate) fn new(inner: RawPooledMut<T>, pool: Rc<RefCell<RawOpaquePool>>) -> Self {
        Self { inner, pool }
    }

    #[doc = include_str!("../../doc/snippets/handle_ptr.md")]
    #[must_use]
    #[inline]
    pub fn ptr(&self) -> NonNull<T> {
        self.inner.ptr()
    }

    #[doc = include_str!("../../doc/snippets/handle_into_shared.md")]
    #[must_use]
    #[inline]
    pub fn into_shared(self) -> LocalPooled<T> {
        let (inner, pool) = self.into_parts();

        LocalPooled::new(inner, pool)
    }

    fn into_parts(self) -> (RawPooledMut<T>, Rc<RefCell<RawOpaquePool>>) {
        // We transfer these fields to the caller, so we do not want the current handle
        // to be dropped. Hence we perform raw reads to extract the fields directly.

        // SAFETY: The target is valid for reads.
        let pool = unsafe { ptr::read(&raw const self.pool) };
        // SAFETY: The target is valid for reads.
        let inner = unsafe { ptr::read(&raw const self.inner) };

        // We are just "destructuring with Drop" here.
        mem::forget(self);

        (inner, pool)
    }

    #[doc = include_str!("../../doc/snippets/ref_counted_as_pin.md")]
    #[must_use]
    #[inline]
    pub fn as_pin(&self) -> Pin<&T> {
        // SAFETY: Pooled items are always pinned.
        unsafe { Pin::new_unchecked(self) }
    }

    #[doc = include_str!("../../doc/snippets/ref_counted_as_pin_mut.md")]
    #[must_use]
    #[inline]
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
    #[inline]
    pub unsafe fn __private_cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> LocalPooledMut<U>
    where
        F: FnOnce(&mut T) -> &mut U,
    {
        let (inner, pool) = self.into_parts();

        // SAFETY: Forwarding callback safety guarantees from the caller.
        // We are an exclusive handle, so we always have the right to create
        // exclusive references to the target of the handle, satisfying that requirement.
        let new_inner = unsafe { inner.__private_cast_dyn_with_fn(cast_fn) };

        LocalPooledMut {
            inner: new_inner,
            pool,
        }
    }
}

impl<T> LocalPooledMut<T>
where
    T: Unpin,
{
    #[doc = include_str!("../../doc/snippets/ref_counted_into_inner.md")]
    #[must_use]
    #[inline]
    pub fn into_inner(self) -> T {
        let (inner, pool) = self.into_parts();

        let mut pool = RefCell::borrow_mut(&pool);
        pool.remove_mut_unpin(inner)
    }
}

impl<T: ?Sized> fmt::Debug for LocalPooledMut<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalPooledMut")
            .field("inner", &self.inner)
            .field("pool", &self.pool)
            .finish()
    }
}

impl<T: ?Sized> Deref for LocalPooledMut<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: This is a unique handle, so we guarantee borrow safety
        // of the target object by borrowing the handle itself.
        // We guarantee liveness by being a reference counted handle.
        unsafe { self.ptr().as_ref() }
    }
}

impl<T> DerefMut for LocalPooledMut<T>
where
    T: ?Sized + Unpin,
{
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: This is a unique handle, so we guarantee borrow safety
        // of the target object by borrowing the handle itself.
        // We guarantee liveness by being a reference counted handle.
        unsafe { self.ptr().as_mut() }
    }
}

impl<T: ?Sized> Borrow<T> for LocalPooledMut<T> {
    #[inline]
    fn borrow(&self) -> &T {
        self
    }
}

impl<T> BorrowMut<T> for LocalPooledMut<T>
where
    T: ?Sized + Unpin,
{
    #[inline]
    fn borrow_mut(&mut self) -> &mut T {
        self
    }
}

impl<T: ?Sized> AsRef<T> for LocalPooledMut<T> {
    #[inline]
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T> AsMut<T> for LocalPooledMut<T>
where
    T: ?Sized + Unpin,
{
    #[inline]
    fn as_mut(&mut self) -> &mut T {
        self
    }
}

impl<T: ?Sized> Drop for LocalPooledMut<T> {
    fn drop(&mut self) {
        // While `RawPooledMut` is technically not Copy, we use our insider knowledge
        // that actually it is in reality just a fat pointer, so we can actually copy it.
        // The only reason it is not Copy is to ensure uniqueness, which we do not care
        // about here because the copy in `self` is going away. We just do not want to
        // insert an Option that we have to check in every method.
        //
        // SAFETY: The target is valid for reads.
        let inner = unsafe { ptr::read(&raw const self.inner) };

        let mut pool = RefCell::borrow_mut(&self.pool);
        pool.remove_mut(inner);
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::*;

    assert_not_impl_any!(LocalPooledMut<u32>: Send, Sync);

    // We expect no destructor because we treat it as `Copy` in our own Drop::drop().
    assert_not_impl_any!(RawPooledMut<()>: Drop);
}
