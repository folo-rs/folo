use std::any::type_name;
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

    // This gives us our thread-safety characteristics (single-threaded),
    // overriding those of `RawPooled<T>`. This is expected because we align
    // with the stricter constraints of the pool itself, even if the underlying
    // slab storage allows for more flexibility.
    remover: Rc<Remover>,
}

impl<T: ?Sized> LocalBlindPooled<T> {
    #[must_use]
    pub(crate) fn new(inner: RawPooledMut<T>, key: LayoutKey, core: LocalBlindPoolCore) -> Self {
        let inner = inner.into_shared();

        let remover = Remover {
            // SAFETY: This handle is single-threaded, no cross-thread access even if `T: Send`.
            handle: unsafe { inner.erase_raw() },
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
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    pub fn ptr(&self) -> NonNull<T> {
        self.inner.ptr()
    }

    #[doc = include_str!("../../doc/snippets/ref_counted_as_pin.md")]
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
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

    /// Erase the type information from this handle, converting it to `LocalBlindPooled<()>`.
    ///
    /// This is useful for extending the lifetime of an object in the pool without retaining
    /// type information. The type-erased handle prevents access to the object but ensures
    /// it remains in the pool.
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // All mutations unviable - save some time.
    pub fn erase(self) -> LocalBlindPooled<()> {
        LocalBlindPooled {
            // SAFETY: This handle is single-threaded, no cross-thread access even if `T: Send`.
            inner: unsafe { self.inner.erase_raw() },
            remover: self.remover,
        }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: ?Sized> fmt::Debug for LocalBlindPooled<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("inner", &self.inner)
            .field("remover", &self.remover)
            .finish()
    }
}

impl<T: ?Sized> Deref for LocalBlindPooled<T> {
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

impl<T: ?Sized> Borrow<T> for LocalBlindPooled<T> {
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants does not understand this signature - every mutation is unviable waste of time.
    fn borrow(&self) -> &T {
        self
    }
}

impl<T: ?Sized> AsRef<T> for LocalBlindPooled<T> {
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants does not understand this signature - every mutation is unviable waste of time.
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
        // that can remove the item from the pool. We keep the pool alive for as long as any
        // handle or remover referencing it exists, so the pool must still exist.
        unsafe {
            pool.remove(self.handle);
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use std::borrow::Borrow;

    use super::*;
    use crate::{LocalBlindPool, NotSendNotSync, NotSendSync, SendAndSync, SendNotSync};

    assert_not_impl_any!(LocalBlindPooled<SendAndSync>: Send, Sync);
    assert_not_impl_any!(LocalBlindPooled<SendNotSync>: Send, Sync);
    assert_not_impl_any!(LocalBlindPooled<NotSendNotSync>: Send, Sync);
    assert_not_impl_any!(LocalBlindPooled<NotSendSync>: Send, Sync);

    // This is a shared handle, must be cloneable.
    assert_impl_all!(LocalBlindPooled<SendAndSync>: Clone);

    assert_not_impl_any!(Remover: Send, Sync);

    // Must have a destructor because we need to remove the object on destroy.
    assert_impl_all!(Remover: Drop);

    #[test]
    fn as_pin_returns_pinned_reference() {
        let pool = LocalBlindPool::new();
        let shared = pool.insert(42_u32).into_shared();

        let pinned = shared.as_pin();
        assert_eq!(*pinned.get_ref(), 42);
    }

    #[test]
    fn deref_returns_reference_to_value() {
        let pool = LocalBlindPool::new();
        let shared = pool.insert(42_u32).into_shared();

        assert_eq!(*shared, 42);
    }

    #[test]
    fn borrow_returns_reference() {
        let pool = LocalBlindPool::new();
        let shared = pool.insert(42_u32).into_shared();

        let borrowed: &u32 = shared.borrow();
        assert_eq!(*borrowed, 42);
    }

    #[test]
    fn as_ref_returns_reference() {
        let pool = LocalBlindPool::new();
        let shared = pool.insert(42_u32).into_shared();

        let reference: &u32 = shared.as_ref();
        assert_eq!(*reference, 42);
    }

    #[test]
    fn erase_extends_lifetime() {
        let pool = LocalBlindPool::new();
        let shared = pool.insert(42_u32).into_shared();

        let erased = shared.clone().erase();

        assert_eq!(pool.len(), 1);
        assert_eq!(*shared, 42);

        drop(shared);
        assert_eq!(pool.len(), 1);

        drop(erased);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn clone_creates_independent_handle() {
        let pool = LocalBlindPool::new();
        let shared = pool.insert(42_u32).into_shared();

        let cloned = shared.clone();
        assert_eq!(*cloned, 42);

        drop(shared);
        assert_eq!(pool.len(), 1);
        assert_eq!(*cloned, 42);

        drop(cloned);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn from_mut_converts_to_shared() {
        let pool = LocalBlindPool::new();
        let handle = pool.insert(42_u32);

        let shared: LocalBlindPooled<u32> = LocalBlindPooled::from(handle);
        assert_eq!(*shared, 42);
        assert_eq!(pool.len(), 1);
    }

    // Must have a destructor because we need to remove the object on destroy.
    assert_impl_all!(Remover: Drop);
}
