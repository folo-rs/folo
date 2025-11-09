use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::{PooledMut, RawOpaquePoolSend, RawPooled, RawPooledMut};

// Note that while this is a thread-safe handle, we do not require `T: Send` because
// we do not want to require every trait we cast into via trait object to be `Send`.
// It is the responsibility of the pool to ensure that only `Send` objects are inserted.

/// A shared thread-safe reference-counting handle for a pooled object.
#[doc = include_str!("../../doc/snippets/ref_counted_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/shared_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/nonlocal_handle_thread_safety.md")]
pub struct Pooled<T: ?Sized> {
    inner: RawPooled<T>,
    remover: Arc<Remover>,
}

impl<T: ?Sized> Pooled<T> {
    #[must_use]
    pub(crate) fn new(inner: RawPooledMut<T>, pool: Arc<Mutex<RawOpaquePoolSend>>) -> Self {
        let inner = inner.into_shared();

        let remover = Remover {
            handle: inner.erase(),
            pool,
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
    #[inline]
    pub unsafe fn __private_cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> Pooled<U>
    where
        F: FnOnce(&T) -> &U,
    {
        // SAFETY: Forwarding callback safety guarantees from the caller.
        // We are a shared handle, so we always have the right to create
        // shared references to the target of the handle, satisfying that requirement.
        let new_inner = unsafe { self.inner.__private_cast_dyn_with_fn(cast_fn) };

        Pooled {
            inner: new_inner,
            remover: self.remover,
        }
    }
}

impl<T: ?Sized> fmt::Debug for Pooled<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pooled")
            .field("inner", &self.inner)
            .field("remover", &self.remover)
            .finish()
    }
}

impl<T: ?Sized> Deref for Pooled<T> {
    type Target = T;

    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: This is a shared handle - the only references
        // that can ever exist are shared references.
        // We guarantee liveness by being a reference counted handle.
        unsafe { self.ptr().as_ref() }
    }
}

impl<T: ?Sized> Borrow<T> for Pooled<T> {
    #[inline]
    fn borrow(&self) -> &T {
        self
    }
}

impl<T: ?Sized> AsRef<T> for Pooled<T> {
    #[inline]
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T: ?Sized> Clone for Pooled<T> {
    #[inline]
    fn clone(&self) -> Self {
        Self {
            inner: self.inner,
            remover: Arc::clone(&self.remover),
        }
    }
}

impl<T: ?Sized> From<PooledMut<T>> for Pooled<T> {
    #[inline]
    fn from(value: PooledMut<T>) -> Self {
        value.into_shared()
    }
}

/// When dropped, removes an object from a pool.
struct Remover {
    handle: RawPooled<()>,
    pool: Arc<Mutex<RawOpaquePoolSend>>,
}

impl fmt::Debug for Remover {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Remover")
            .field("handle", &self.handle)
            .field("pool", &"<pool>")
            .finish()
    }
}

impl Drop for Remover {
    fn drop(&mut self) {
        let mut pool = self.pool.lock();

        // SAFETY: The remover controls the shared object lifetime and is the only thing
        // that can remove the item from the pool.
        unsafe {
            pool.remove_unchecked(self.handle);
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
    use std::cell::Cell;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    // u32 is Sync, so Pooled<u32> should be Send (but not Sync).
    assert_impl_all!(Pooled<u32>: Send);
    assert_not_impl_any!(Pooled<u32>: Sync);

    // Cell is Send but not Sync, so Pooled<Cell> should be neither Send nor Sync.
    assert_not_impl_any!(Pooled<Cell<u32>>: Send, Sync);
}
