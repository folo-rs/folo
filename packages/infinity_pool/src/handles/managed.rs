use std::any::type_name;
use std::borrow::Borrow;
use std::fmt;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::{PooledMut, RawOpaquePoolThreadSafe, RawPooled, RawPooledMut};

// Note that while this is a thread-safe handle, we do not require `T: Send` because
// we do not want to require every trait we cast into via trait object to be `Send`.
// It is the responsibility of the pool to ensure that only `Send` objects are inserted.

/// A shared thread-safe reference-counting handle for a pooled object.
#[doc = include_str!("../../doc/snippets/ref_counted_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/shared_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/nonlocal_handle_thread_safety.md")]
pub struct Pooled<T: ?Sized> {
    // We inherit our thread-safety traits from this one (Send from T, Sync always).
    inner: RawPooled<T>,

    remover: Arc<Remover>,
}

impl<T: ?Sized> Pooled<T> {
    /// # Safety
    ///
    /// Even though the signature does not require `T: Send`, the underlying object must be `Send`.
    /// The signature does not require it to be compatible with casting to trait objects that do
    /// not have `Send` as a supertrait.
    #[must_use]
    pub(crate) unsafe fn new(
        inner: RawPooledMut<T>,
        pool: Arc<Mutex<RawOpaquePoolThreadSafe>>,
    ) -> Self {
        let inner = inner.into_shared();

        let remover = Remover {
            // SAFETY: This is a thread-safe handle, which means it can only work on Send types,
            // so we can have no risk of `T: !Send` which is the main thing we worry about when
            // erasing the object type. No issue with `Send` types at all.
            handle: unsafe { inner.erase_raw() },
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

    /// Erase the type information from this handle, converting it to `Pooled<()>`.
    ///
    /// This is useful for extending the lifetime of an object in the pool without retaining
    /// type information. The type-erased handle prevents access to the object but ensures
    /// it remains in the pool.
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // All mutations unviable - save some time.
    pub fn erase(self) -> Pooled<()> {
        Pooled {
            // SAFETY: This is a thread-safe handle, which means it can only work on Send types,
            // so we can have no risk of `T: !Send` which is the main thing we worry about when
            // erasing the object type. No issue with `Send` types at all.
            inner: unsafe { self.inner.erase_raw() },
            remover: self.remover,
        }
    }
}

impl<T: ?Sized> fmt::Debug for Pooled<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("inner", &self.inner)
            .field("remover", &self.remover)
            .finish()
    }
}

impl<T: ?Sized> Deref for Pooled<T> {
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

impl<T: ?Sized> Borrow<T> for Pooled<T> {
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants does not understand this signature - every mutation is unviable waste of time.
    fn borrow(&self) -> &T {
        self
    }
}

impl<T: ?Sized> AsRef<T> for Pooled<T> {
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants does not understand this signature - every mutation is unviable waste of time.
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
    pool: Arc<Mutex<RawOpaquePoolThreadSafe>>,
}

impl fmt::Debug for Remover {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
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
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::{NotSendNotSync, NotSendSync, SendAndSync, SendNotSync};

    assert_impl_all!(Pooled<SendAndSync>: Send, Sync);
    assert_impl_all!(Pooled<SendNotSync>: Send, Sync);
    assert_impl_all!(Pooled<NotSendNotSync>: Sync);
    assert_impl_all!(Pooled<NotSendSync>: Sync);

    assert_not_impl_any!(Pooled<NotSendNotSync>: Send);
    assert_not_impl_any!(Pooled<NotSendSync>: Send);

    // This is a shared handle, must be cloneable.
    assert_impl_all!(Pooled<SendAndSync>: Clone);

    assert_impl_all!(Remover: Send, Sync);

    // Must have a destructor because we need to remove the object on destroy.
    assert_impl_all!(Remover: Drop);

    #[test]
    fn erase_extends_lifetime() {
        use crate::OpaquePool;

        let pool = OpaquePool::with_layout_of::<u32>();
        let handle = pool.insert(42);
        let shared = handle.into_shared();

        // Clone one handle and erase it.
        let erased = shared.clone().erase();

        // Both handles keep the object alive.
        assert_eq!(pool.len(), 1);
        assert_eq!(*shared, 42);

        // Drop the typed handle.
        drop(shared);

        // Object still alive due to erased handle.
        assert_eq!(pool.len(), 1);

        // Drop erased handle, object is removed.
        drop(erased);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn erase_multiple_clones() {
        use crate::OpaquePool;

        let pool = OpaquePool::with_layout_of::<String>();
        let handle = pool.insert(String::from("test"));
        let shared = handle.into_shared();

        let clone1 = shared.clone();
        let clone2 = shared.clone();
        let erased1 = clone1.erase();
        let erased2 = clone2.erase();

        assert_eq!(pool.len(), 1);
        assert_eq!(*shared, "test");

        drop(shared);
        assert_eq!(pool.len(), 1);

        drop(erased1);
        assert_eq!(pool.len(), 1);

        drop(erased2);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn erase_works_with_not_unpin_types() {
        use std::marker::PhantomPinned;

        use crate::OpaquePool;

        // Type that is Send + Sync but !Unpin
        struct NotUnpin {
            #[allow(dead_code, reason = "Field used to give struct non-zero size")]
            data: i32,
            _marker: PhantomPinned,
        }

        let pool = OpaquePool::with_layout_of::<NotUnpin>();
        let handle = pool.insert(NotUnpin {
            data: 42,
            _marker: PhantomPinned,
        });

        // Erasing !Unpin types now works because the compile-time assertion in
        // remove_unpin() prevents calling it with ().
        let erased = handle.into_shared().erase();

        assert_eq!(pool.len(), 1);

        drop(erased);
        assert_eq!(pool.len(), 0);
    }
}
