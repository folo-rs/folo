use std::borrow::{Borrow, BorrowMut};
use std::cell::RefCell;
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::Rc;
use std::{fmt, mem, ptr};

use crate::{LocalPooled, RawOpaquePool, RawPooledMut};

/// A unique handle to a reference-counted object in an object pool.
///
/// The handle can be used to access the pooled object, as well as to remove
/// the item from the pool when no longer needed.
///
/// This is a reference-counted handle that automatically removes the object when the
/// handle is dropped. Dropping the handle is the only way to remove the object from
/// the pool.
///
/// This is a unique handle, guaranteeing that no other handles to the same object exist. You may
/// create both shared and exclusive references to the object through this handle. The handle may
/// also be converted to a shared handle via `.into_shared()`.
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
    pub fn erase(self) -> LocalPooledMut<()> {
        let (inner, pool) = self.into_parts();

        LocalPooledMut {
            inner: inner.erase(),
            pool,
        }
    }

    /// Transforms the unique handle into a shared handle that can be cloned and copied freely.
    #[must_use]
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

impl<T: Unpin> LocalPooledMut<T> {
    /// Removes the item from the pool and returns it to the caller.
    #[must_use]
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

    fn deref(&self) -> &Self::Target {
        // SAFETY: This is a unique handle, so we guarantee borrow safety
        // of the target object by borrowing the handle itself.
        unsafe { self.ptr().as_ref() }
    }
}

impl<T: ?Sized + Unpin> DerefMut for LocalPooledMut<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: This is a unique handle, so we guarantee borrow safety
        // of the target object by borrowing the handle itself.
        unsafe { self.ptr().as_mut() }
    }
}

impl<T: ?Sized> Borrow<T> for LocalPooledMut<T> {
    fn borrow(&self) -> &T {
        self
    }
}

impl<T: ?Sized + Unpin> BorrowMut<T> for LocalPooledMut<T> {
    fn borrow_mut(&mut self) -> &mut T {
        self
    }
}

impl<T: ?Sized> AsRef<T> for LocalPooledMut<T> {
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T: ?Sized + Unpin> AsMut<T> for LocalPooledMut<T> {
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
}
