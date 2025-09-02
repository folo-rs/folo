// Note that we do not require `T: Send` because we do not want to require every
// trait we cast into via trait object to be `Send`. It is the responsibility of
// the pool to ensure that only `Send` objects are inserted.

use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use crate::{ERR_POISONED_LOCK, PooledMut, RawOpaquePoolSend, RawPooled, RawPooledMut};

/// A shared handle to a reference-counted object in an object pool.
///
/// The handle can be used to access the pooled object, as well as to remove
/// the item from the pool when no longer needed.
///
/// This is a reference-counted handle that automatically removes the object when the
/// last clone of the handle is dropped. Dropping the handle is the only way to remove
/// the object from the pool.
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
    pub fn erase(self) -> Pooled<()> {
        Pooled {
            inner: self.inner.erase(),
            remover: self.remover,
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

    fn deref(&self) -> &Self::Target {
        // SAFETY: This is a shared handle - the only references
        // that can ever exist are shared references.
        unsafe { self.ptr().as_ref() }
    }
}

impl<T: ?Sized> Borrow<T> for Pooled<T> {
    fn borrow(&self) -> &T {
        self
    }
}

impl<T: ?Sized> AsRef<T> for Pooled<T> {
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T: ?Sized> Clone for Pooled<T> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner,
            remover: Arc::clone(&self.remover),
        }
    }
}

impl<T: ?Sized> From<PooledMut<T>> for Pooled<T> {
    fn from(value: PooledMut<T>) -> Self {
        value.into_shared()
    }
}

/// When dropped, removes an object from a pool.
#[derive(Debug)]
struct Remover {
    handle: RawPooled<()>,
    pool: Arc<Mutex<RawOpaquePoolSend>>,
}

impl Drop for Remover {
    fn drop(&mut self) {
        let mut pool = self.pool.lock().expect(ERR_POISONED_LOCK);

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
    use std::cell::Cell;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    // u32 is Sync, so Pooled<u32> should be Send (but not Sync).
    assert_impl_all!(Pooled<u32>: Send);
    assert_not_impl_any!(Pooled<u32>: Sync);

    // Cell is Send but not Sync, so Pooled<Cell> should be neither Send nor Sync.
    assert_not_impl_any!(Pooled<Cell<u32>>: Send, Sync);
}
