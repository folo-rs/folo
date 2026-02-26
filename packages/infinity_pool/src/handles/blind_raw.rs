use std::any::type_name;
use std::fmt;
use std::pin::Pin;
use std::ptr::NonNull;

use crate::{LayoutKey, RawBlindPooledMut, RawPooled};

/// A shared handle to an object in a [`RawBlindPool`][crate::RawBlindPool].
#[doc = include_str!("../../doc/snippets/raw_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/shared_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/shared_raw_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/nonlocal_handle_thread_safety.md")]
pub struct RawBlindPooled<T>
where
    // We support casting to trait objects, hence `?Sized`.
    T: ?Sized,
{
    key: LayoutKey,

    // We inherit our thread-safety traits from this one (Send from T, Sync always).
    inner: RawPooled<T>,
}

impl<T: ?Sized> RawBlindPooled<T> {
    #[must_use]
    pub(crate) fn new(key: LayoutKey, inner: RawPooled<T>) -> Self {
        Self { key, inner }
    }

    /// The layout key used to identify the inner pool the blind pool used to store it.
    #[must_use]
    pub(crate) fn layout_key(&self) -> LayoutKey {
        self.key
    }

    /// Becomes the inner handle for the `RawOpaquePool` that holds the object.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    pub(crate) fn into_inner(self) -> RawPooled<T> {
        self.inner
    }

    #[doc = include_str!("../../doc/snippets/handle_ptr.md")]
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    pub fn ptr(&self) -> NonNull<T> {
        self.inner.ptr()
    }

    #[doc = include_str!("../../doc/snippets/raw_as_pin.md")]
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    pub unsafe fn as_pin(&self) -> Pin<&T> {
        // SAFETY: Forwarding safety guarantees from the caller.
        let as_ref = unsafe { self.as_ref() };

        // SAFETY: Pooled items are always pinned.
        unsafe { Pin::new_unchecked(as_ref) }
    }

    #[doc = include_str!("../../doc/snippets/raw_as_ref.md")]
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    pub unsafe fn as_ref(&self) -> &T {
        // SAFETY: This is a unique handle, so we guarantee borrow safety
        // of the target object by borrowing the handle itself. Pointer validity
        // requires pool to be alive, which is a safety requirement of this function.
        unsafe { self.ptr().as_ref() }
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
    pub unsafe fn __private_cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> RawBlindPooled<U>
    where
        F: FnOnce(&T) -> &U,
    {
        // SAFETY: Forwarding callback safety guarantees from the caller.
        // We are a shared handle, so we always have the right to create
        // shared references to the target of the handle, satisfying that requirement.
        let new_inner = unsafe { self.inner.__private_cast_dyn_with_fn(cast_fn) };

        RawBlindPooled {
            key: self.key,
            inner: new_inner,
        }
    }

    /// Erase the type information from this handle, converting it to `RawBlindPooled<()>`.
    ///
    /// This is useful for extending the lifetime of an object in the pool without retaining
    /// type information. The type-erased handle prevents access to the object but ensures
    /// it remains in the pool.
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // All mutations unviable - save some time.
    pub fn erase(self) -> RawBlindPooled<()>
    where
        T: Send,
    {
        RawBlindPooled {
            key: self.key,
            // SAFETY: The risk is that if `T: !Send` then `Self<()>` would be `Send`. Calling
            // `pool.remove()` with this handle would move the object across threads to drop it.
            // Therefore, we cannot allow type-erased blind handles for `!Send` types.
            // This only affects blind pools because typed pools enforce this on a pool level,
            // forbidding the pool itself to be accessed across threads if `T: !Send`.
            // We are safe because this method has the additional bound `T: Send`.
            inner: unsafe { self.inner.erase_raw() },
        }
    }
}

impl<T: ?Sized> Clone for RawBlindPooled<T> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> Copy for RawBlindPooled<T> {}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: ?Sized> fmt::Debug for RawBlindPooled<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("key", &self.key)
            .field("inner", &self.inner)
            .finish()
    }
}

impl<T: ?Sized> From<RawBlindPooledMut<T>> for RawBlindPooled<T> {
    #[inline]
    fn from(value: RawBlindPooledMut<T>) -> Self {
        value.into_shared()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::{NotSendNotSync, NotSendSync, RawBlindPool, SendAndSync, SendNotSync};

    assert_impl_all!(RawBlindPooled<SendAndSync>: Send, Sync);
    assert_impl_all!(RawBlindPooled<SendNotSync>: Send, Sync);
    assert_impl_all!(RawBlindPooled<NotSendNotSync>: Sync);
    assert_impl_all!(RawBlindPooled<NotSendSync>:  Sync);

    assert_not_impl_any!(RawBlindPooled<NotSendNotSync>: Send);
    assert_not_impl_any!(RawBlindPooled<NotSendSync>: Send);

    // Shared raw handles are just fancy pointers, value objects.
    assert_impl_all!(RawBlindPooled<SendAndSync>: Copy);
    assert_not_impl_any!(RawBlindPooled<SendAndSync>: Drop);

    #[test]
    fn as_pin_returns_pinned_reference() {
        let mut pool = RawBlindPool::new();
        let handle = pool.insert(42_u32);
        let shared = handle.into_shared();

        // SAFETY: Handle is valid and pool is still alive.
        let pinned = unsafe { shared.as_pin() };
        assert_eq!(*pinned.get_ref(), 42);
    }

    #[test]
    fn as_ref_returns_reference() {
        let mut pool = RawBlindPool::new();
        let handle = pool.insert(42_u32);
        let shared = handle.into_shared();

        // SAFETY: Handle is valid and pool is still alive.
        let reference = unsafe { shared.as_ref() };
        assert_eq!(*reference, 42);
    }

    #[test]
    fn clone_creates_copy() {
        let mut pool = RawBlindPool::new();
        let handle = pool.insert(42_u32);
        let shared = handle.into_shared();

        let cloned = shared;
        let copied = shared;
        #[expect(clippy::clone_on_copy, reason = "explicitly testing Clone impl")]
        let explicitly_cloned = shared.clone();

        // SAFETY: Handles are valid and pool is still alive.
        assert_eq!(unsafe { *cloned.as_ref() }, 42);
        // SAFETY: Handles are valid and pool is still alive.
        assert_eq!(unsafe { *copied.as_ref() }, 42);
        // SAFETY: Handles are valid and pool is still alive.
        assert_eq!(unsafe { *explicitly_cloned.as_ref() }, 42);
    }

    #[test]
    fn erase_creates_type_erased_handle() {
        let mut pool = RawBlindPool::new();
        let handle = pool.insert(42_u32);
        let shared = handle.into_shared();

        let erased = shared.erase();

        assert_eq!(pool.len(), 1);

        // SAFETY: Handle is valid and pool is still alive.
        unsafe {
            pool.remove(erased);
        }
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn from_mut_converts_to_shared() {
        let mut pool = RawBlindPool::new();
        let handle = pool.insert(42_u32);

        let shared: RawBlindPooled<u32> = RawBlindPooled::from(handle);

        // SAFETY: Handle is valid and pool is still alive.
        assert_eq!(unsafe { *shared.as_ref() }, 42);
    }
}
