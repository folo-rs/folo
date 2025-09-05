use std::borrow::Borrow;
use std::cell::RefCell;
use std::fmt;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::Rc;

use crate::{LocalPooledMut, RawOpaquePool, RawPooled, RawPooledMut};

/// A shared single-threaded reference-counting handle for a pooled object.
#[doc = include_str!("../../doc/snippets/ref_counted_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/shared_handle_implications.md")]
///
/// # Thread safety
///
/// This type is single-threaded.
pub struct LocalPooled<T: ?Sized> {
    inner: RawPooled<T>,
    remover: Rc<Remover>,
}

impl<T: ?Sized> LocalPooled<T> {
    #[must_use]
    pub(crate) fn new(inner: RawPooledMut<T>, pool: Rc<RefCell<RawOpaquePool>>) -> Self {
        let inner = inner.into_shared();

        let remover = Remover {
            handle: inner.erase(),
            pool,
        };

        Self {
            inner,
            remover: Rc::new(remover),
        }
    }

    #[doc = include_str!("../../doc/snippets/handle_ptr.md")]
    #[must_use]
    pub fn ptr(&self) -> NonNull<T> {
        self.inner.ptr()
    }

    #[doc = include_str!("../../doc/snippets/handle_erase.md")]
    #[must_use]
    pub fn erase(self) -> LocalPooled<()> {
        LocalPooled {
            inner: self.inner.erase(),
            remover: self.remover,
        }
    }

    #[doc = include_str!("../../doc/snippets/ref_counted_as_pin.md")]
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
    pub unsafe fn __private_cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> LocalPooled<U>
    where
        F: FnOnce(&T) -> &U,
    {
        // SAFETY: Forwarding callback safety guarantees from the caller.
        // We are a shared handle, so we always have the right to create
        // shared references to the target of the handle, satisfying that requirement.
        let new_inner = unsafe { self.inner.__private_cast_dyn_with_fn(cast_fn) };

        LocalPooled {
            inner: new_inner,
            remover: self.remover,
        }
    }
}

impl<T: ?Sized> fmt::Debug for LocalPooled<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalPooled")
            .field("inner", &self.inner)
            .field("remover", &self.remover)
            .finish()
    }
}

impl<T: ?Sized> Deref for LocalPooled<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: This is a shared handle - the only references
        // that can ever exist are shared references.
        // We guarantee liveness by being a reference counted handle.
        unsafe { self.ptr().as_ref() }
    }
}

impl<T: ?Sized> Borrow<T> for LocalPooled<T> {
    fn borrow(&self) -> &T {
        self
    }
}

impl<T: ?Sized> AsRef<T> for LocalPooled<T> {
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T: ?Sized> Clone for LocalPooled<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner,
            remover: Rc::clone(&self.remover),
        }
    }
}

impl<T: ?Sized> From<LocalPooledMut<T>> for LocalPooled<T> {
    fn from(value: LocalPooledMut<T>) -> Self {
        value.into_shared()
    }
}

/// When dropped, removes an object from a pool.
#[derive(Debug)]
struct Remover {
    handle: RawPooled<()>,
    pool: Rc<RefCell<RawOpaquePool>>,
}

impl Drop for Remover {
    fn drop(&mut self) {
        let mut pool = self.pool.borrow_mut();

        // SAFETY: The remover controls the shared object lifetime and is the only thing
        // that can remove the item from the pool.
        unsafe {
            pool.remove(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::*;

    assert_not_impl_any!(LocalPooled<u32>: Send, Sync);
}
