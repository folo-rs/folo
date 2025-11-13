use std::any::type_name;
use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::Arc;

use crate::{BlindPoolCore, BlindPooledMut, LayoutKey, RawPooled, RawPooledMut};

// Note that while this is a thread-safe handle, we do not require `T: Send` because
// we do not want to require every trait we cast into via trait object to be `Send`.
// It is the responsibility of the pool to ensure that only `Send` objects are inserted.

/// A shared thread-safe reference-counting handle for a pooled object.
#[doc = include_str!("../../doc/snippets/ref_counted_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/shared_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/nonlocal_handle_thread_safety.md")]
pub struct BlindPooled<T: ?Sized> {
    // We inherit our thread-safety traits from this one (Send from T, Sync always).
    inner: RawPooled<T>,

    remover: Arc<Remover>,
}

impl<T: ?Sized> BlindPooled<T> {
    /// # Safety
    ///
    /// Even though the signature does not require `T: Send`, the underlying object must be `Send`.
    /// The signature does not require it to be compatible with casting to trait objects that do
    /// not have `Send` as a supertrait.
    #[must_use]
    pub(crate) unsafe fn new(inner: RawPooledMut<T>, key: LayoutKey, core: BlindPoolCore) -> Self {
        let inner = inner.into_shared();

        let remover = Remover {
            // SAFETY: This is a thread-safe handle, which means it can only work on Send types,
            // so we can have no risk of `T: !Send` which is the main thing we worry about when
            // erasing the object type. No issue with `Send` types at all.
            handle: unsafe { inner.erase_raw() },
            key,
            core,
        };

        Self {
            inner,
            remover: Arc::new(remover),
        }
    }

    #[doc = include_str!("../../doc/snippets/handle_ptr.md")]
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    pub fn ptr(&self) -> NonNull<T> {
        self.inner.ptr()
    }

    #[doc = include_str!("../../doc/snippets/ref_counted_as_pin.md")]
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    pub fn as_pin(&self) -> Pin<&T> {
        // SAFETY: BlindPooled items are always pinned.
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
    pub unsafe fn __private_cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> BlindPooled<U>
    where
        F: FnOnce(&T) -> &U,
    {
        // SAFETY: Forwarding callback safety guarantees from the caller.
        // We are a shared handle, so we always have the right to create
        // shared references to the target of the handle, satisfying that requirement.
        let new_inner = unsafe { self.inner.__private_cast_dyn_with_fn(cast_fn) };

        BlindPooled {
            inner: new_inner,
            remover: self.remover,
        }
    }

    /// Erase the type information from this handle, converting it to `BlindPooled<()>`.
    ///
    /// This is useful for extending the lifetime of an object in the pool without retaining
    /// type information. The type-erased handle prevents access to the object but ensures
    /// it remains in the pool.
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // All mutations unviable - save some time.
    pub fn erase(self) -> BlindPooled<()> {
        BlindPooled {
            // SAFETY: This is a thread-safe handle, which means it can only work on Send types,
            // so we can have no risk of `T: !Send` which is the main thing we worry about when
            // erasing the object type. No issue with `Send` types at all.
            inner: unsafe { self.inner.erase_raw() },
            remover: self.remover,
        }
    }
}

impl<T: ?Sized> fmt::Debug for BlindPooled<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("inner", &self.inner)
            .field("remover", &self.remover)
            .finish()
    }
}

impl<T: ?Sized> Deref for BlindPooled<T> {
    type Target = T;

    #[inline]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants does not understand this signature - every mutation is unviable waste of time.
    fn deref(&self) -> &Self::Target {
        // SAFETY: This is a shared handle - the only references
        // that can ever exist are shared references.
        // We guarantee liveness by being a reference counted handle.
        unsafe { self.ptr().as_ref() }
    }
}

impl<T: ?Sized> Borrow<T> for BlindPooled<T> {
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants does not understand this signature - every mutation is unviable waste of time.
    fn borrow(&self) -> &T {
        self
    }
}

impl<T: ?Sized> AsRef<T> for BlindPooled<T> {
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants does not understand this signature - every mutation is unviable waste of time.
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T: ?Sized> Clone for BlindPooled<T> {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            inner: self.inner,
            remover: Arc::clone(&self.remover),
        }
    }
}

impl<T: ?Sized> From<BlindPooledMut<T>> for BlindPooled<T> {
    #[inline]
    fn from(value: BlindPooledMut<T>) -> Self {
        value.into_shared()
    }
}

/// When dropped, removes an object from a pool.
#[derive(Debug)]
struct Remover {
    handle: RawPooled<()>,
    key: LayoutKey,
    core: BlindPoolCore,
}

impl Drop for Remover {
    fn drop(&mut self) {
        let mut core = self.core.lock();

        let pool = core
            .get_mut(&self.key)
            .expect("if the handle still exists, the inner pool must still exist");

        // SAFETY: The remover controls the shared object lifetime and is the only thing
        // that can remove the item from the pool.
        unsafe {
            pool.remove_unchecked(self.handle);
        }
    }
}

#[cfg(test)]
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::{NotSendNotSync, NotSendSync, SendAndSync, SendNotSync};

    assert_impl_all!(BlindPooled<SendAndSync>: Send, Sync);
    assert_impl_all!(BlindPooled<SendNotSync>: Send, Sync);
    assert_impl_all!(BlindPooled<NotSendNotSync>: Sync);
    assert_impl_all!(BlindPooled<NotSendSync>: Sync);

    assert_not_impl_any!(BlindPooled<NotSendNotSync>: Send);
    assert_not_impl_any!(BlindPooled<NotSendSync>: Send);

    // This is a shared handle, must be cloneable.
    assert_impl_all!(BlindPooled<SendAndSync>: Clone);

    assert_impl_all!(Remover: Send, Sync);

    // Must have a destructor because we need to remove the object on destroy.
    assert_impl_all!(Remover: Drop);
}
