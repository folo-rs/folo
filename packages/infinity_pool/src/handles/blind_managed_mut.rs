use std::borrow::{Borrow, BorrowMut};
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::ptr::NonNull;
use std::{fmt, mem, ptr};

use crate::{BlindPoolCore, BlindPooled, ERR_POISONED_LOCK, LayoutKey, RawPooledMut};

// Note that while this is a thread-safe handle, we do not require `T: Send` because
// we do not want to require every trait we cast into via trait object to be `Send`.
// It is the responsibility of the pool to ensure that only `Send` objects are inserted.

/// A unique thread-safe reference-counting handle for a pooled object.
#[doc = include_str!("../../doc/snippets/ref_counted_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/unique_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/nonlocal_handle_thread_safety.md")]
pub struct BlindPooledMut<T: ?Sized> {
    inner: RawPooledMut<T>,
    key: LayoutKey,
    core: BlindPoolCore,
}

impl<T: ?Sized> BlindPooledMut<T> {
    #[must_use]
    pub(crate) fn new(inner: RawPooledMut<T>, key: LayoutKey, core: BlindPoolCore) -> Self {
        Self { inner, key, core }
    }

    #[doc = include_str!("../../doc/snippets/handle_ptr.md")]
    #[must_use]
    #[inline]
    pub fn ptr(&self) -> NonNull<T> {
        self.inner.ptr()
    }

    #[doc = include_str!("../../doc/snippets/handle_erase.md")]
    #[must_use]
    #[inline]
    pub fn erase(self) -> BlindPooledMut<()> {
        let (inner, key, core) = self.into_parts();

        BlindPooledMut {
            inner: inner.erase(),
            key,
            core,
        }
    }

    #[doc = include_str!("../../doc/snippets/handle_into_shared.md")]
    #[must_use]
    #[inline]
    pub fn into_shared(self) -> BlindPooled<T> {
        let (inner, key, core) = self.into_parts();

        BlindPooled::new(inner, key, core)
    }

    fn into_parts(self) -> (RawPooledMut<T>, LayoutKey, BlindPoolCore) {
        // We transfer these fields to the caller, so we do not want the current handle
        // to be dropped. Hence we perform raw reads to extract the fields directly.

        // SAFETY: The target is valid for reads.
        let inner = unsafe { ptr::read(&raw const self.inner) };
        // SAFETY: The target is valid for reads.
        let key = unsafe { ptr::read(&raw const self.key) };
        // SAFETY: The target is valid for reads.
        let core = unsafe { ptr::read(&raw const self.core) };

        // We are just "destructuring with Drop" here.
        mem::forget(self);

        (inner, key, core)
    }

    #[doc = include_str!("../../doc/snippets/ref_counted_as_pin.md")]
    #[must_use]
    #[inline]
    pub fn as_pin(&self) -> Pin<&T> {
        // SAFETY: BlindPooled items are always pinned.
        unsafe { Pin::new_unchecked(self) }
    }

    #[doc = include_str!("../../doc/snippets/ref_counted_as_pin_mut.md")]
    #[must_use]
    #[inline]
    pub fn as_pin_mut(&mut self) -> Pin<&mut T> {
        // SAFETY: This is a unique handle, so we guarantee borrow safety
        // of the target object by borrowing the handle itself.
        let as_mut = unsafe { self.ptr().as_mut() };

        // SAFETY: BlindPooled items are always pinned.
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
    pub unsafe fn __private_cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> BlindPooledMut<U>
    where
        F: FnOnce(&mut T) -> &mut U,
    {
        let (inner, key, core) = self.into_parts();

        // SAFETY: Forwarding callback safety guarantees from the caller.
        // We are an exclusive handle, so we always have the right to create
        // exclusive references to the target of the handle, satisfying that requirement.
        let new_inner = unsafe { inner.__private_cast_dyn_with_fn(cast_fn) };

        BlindPooledMut {
            inner: new_inner,
            key,
            core,
        }
    }
}

impl<T> BlindPooledMut<T>
where
    T: Unpin,
{
    #[doc = include_str!("../../doc/snippets/ref_counted_into_inner.md")]
    #[must_use]
    #[inline]
    pub fn into_inner(self) -> T {
        let (inner, key, core) = self.into_parts();

        let mut core = core.lock().expect(ERR_POISONED_LOCK);

        let pool = core
            .get_mut(&key)
            .expect("if the handle still exists, the inner pool must still exist");

        pool.remove_mut_unpin(inner)
    }
}

impl<T: ?Sized> fmt::Debug for BlindPooledMut<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlindPooledMut")
            .field("inner", &self.inner)
            .field("key", &self.key)
            .field("core", &self.core)
            .finish()
    }
}

impl<T: ?Sized> Deref for BlindPooledMut<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: This is a unique handle, so we guarantee borrow safety
        // of the target object by borrowing the handle itself.
        // We guarantee liveness by being a reference counted handle.
        unsafe { self.ptr().as_ref() }
    }
}

impl<T> DerefMut for BlindPooledMut<T>
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

impl<T: ?Sized> Borrow<T> for BlindPooledMut<T> {
    #[inline]
    fn borrow(&self) -> &T {
        self
    }
}

impl<T> BorrowMut<T> for BlindPooledMut<T>
where
    T: ?Sized + Unpin,
{
    #[inline]
    fn borrow_mut(&mut self) -> &mut T {
        self
    }
}

impl<T: ?Sized> AsRef<T> for BlindPooledMut<T> {
    #[inline]
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T> AsMut<T> for BlindPooledMut<T>
where
    T: ?Sized + Unpin,
{
    #[inline]
    fn as_mut(&mut self) -> &mut T {
        self
    }
}

impl<T: ?Sized> Drop for BlindPooledMut<T> {
    fn drop(&mut self) {
        // While `RawBlindPooledMut` is technically not Copy, we use our insider knowledge
        // that actually it is in reality just a fat pointer, so we can actually copy it.
        // The only reason it is not Copy is to ensure uniqueness, which we do not care
        // about here because the copy in `self` is going away. We just do not want to
        // insert an Option that we have to check in every method.
        //
        // SAFETY: The target is valid for reads.
        let inner = unsafe { ptr::read(&raw const self.inner) };

        let mut core = self.core.lock().expect(ERR_POISONED_LOCK);

        let pool = core
            .get_mut(&self.key)
            .expect("if the handle still exists, the inner pool must still exist");

        pool.remove_mut(inner);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    // u32 is Sync, so BlindPooledMut<u32> should be Send (but not Sync).
    assert_impl_all!(BlindPooledMut<u32>: Send);
    assert_not_impl_any!(BlindPooledMut<u32>: Sync);

    // Cell is Send but not Sync, so BlindPooledMut<Cell> should be neither Send nor Sync.
    assert_not_impl_any!(BlindPooledMut<Cell<u32>>: Send, Sync);

    // We expect no destructor because we treat it as `Copy` in our own Drop::drop().
    assert_not_impl_any!(RawPooledMut<()>: Drop);
}
