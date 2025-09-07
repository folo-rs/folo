use std::borrow::Borrow;
use std::cell::RefCell;
use std::fmt;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::rc::Rc;

use crate::{LayoutKey, LocalBlindPoolCore, LocalBlindPooledMut, RawPooled, RawPooledMut};

/// A shared single-threaded reference-counting handle for a pooled object.
#[doc = include_str!("../../doc/snippets/ref_counted_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/shared_handle_implications.md")]
///
/// # Thread safety
///
/// This type is single-threaded.
pub struct LocalBlindPooled<T: ?Sized> {
    inner: RawPooled<T>,
    remover: Rc<Remover>,
}

impl<T: ?Sized> LocalBlindPooled<T> {
    #[must_use]
    pub(crate) fn new(inner: RawPooledMut<T>, key: LayoutKey, core: LocalBlindPoolCore) -> Self {
        let inner = inner.into_shared();

        let remover = Remover {
            handle: inner.erase(),
            key,
            core,
        };

        Self {
            inner,
            remover: Rc::new(remover),
        }
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
    pub fn erase(self) -> LocalBlindPooled<()> {
        LocalBlindPooled {
            inner: self.inner.erase(),
            remover: self.remover,
        }
    }

    #[doc = include_str!("../../doc/snippets/ref_counted_as_pin.md")]
    #[must_use]
    #[inline]
    pub fn as_pin(&self) -> Pin<&T> {
        // SAFETY: LocalBlindPooled items are always pinned.
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
    #[inline]
    pub unsafe fn __private_cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> LocalBlindPooled<U>
    where
        F: FnOnce(&T) -> &U,
    {
        // SAFETY: Forwarding callback safety guarantees from the caller.
        // We are a shared handle, so we always have the right to create
        // shared references to the target of the handle, satisfying that requirement.
        let new_inner = unsafe { self.inner.__private_cast_dyn_with_fn(cast_fn) };

        LocalBlindPooled {
            inner: new_inner,
            remover: self.remover,
        }
    }
}

impl<T: ?Sized> fmt::Debug for LocalBlindPooled<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalBlindPooled")
            .field("inner", &self.inner)
            .field("remover", &self.remover)
            .finish()
    }
}

impl<T: ?Sized> Deref for LocalBlindPooled<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: This is a shared handle - the only references
        // that can ever exist are shared references.
        // We guarantee liveness by being a reference counted handle.
        unsafe { self.ptr().as_ref() }
    }
}

impl<T: ?Sized> Borrow<T> for LocalBlindPooled<T> {
    #[inline]
    fn borrow(&self) -> &T {
        self
    }
}

impl<T: ?Sized> AsRef<T> for LocalBlindPooled<T> {
    #[inline]
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T: ?Sized> Clone for LocalBlindPooled<T> {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            inner: self.inner,
            remover: Rc::clone(&self.remover),
        }
    }
}

impl<T: ?Sized> From<LocalBlindPooledMut<T>> for LocalBlindPooled<T> {
    #[inline]
    fn from(value: LocalBlindPooledMut<T>) -> Self {
        value.into_shared()
    }
}

/// When dropped, removes an object from a pool.
#[derive(Debug)]
struct Remover {
    handle: RawPooled<()>,
    key: LayoutKey,
    core: LocalBlindPoolCore,
}

impl Drop for Remover {
    fn drop(&mut self) {
        let mut core = RefCell::borrow_mut(&self.core);

        let pool = core
            .get_mut(&self.key)
            .expect("if the handle still exists, the inner pool must still exist");

        // SAFETY: The remover controls the shared object lifetime and is the only thing
        // that can remove the item from the pool.
        unsafe {
            pool.remove(self.handle);
        }
    }
}

// SAFETY: By default we do not have `Sync` because the handle is not `Sync`. However, the reason
// for that is because the handle can be used to access the object. As we have a type-erased handle
// that cannot access anything meaningful, and as the remover is not accessing the object anyway,
// just removing it from the pool, we can safety glue back the `Sync` label onto the type.
unsafe impl Sync for Remover {}

#[cfg(test)]
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::*;

    assert_not_impl_any!(LocalBlindPooled<u32>: Send, Sync);
}
