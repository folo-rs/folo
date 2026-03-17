use std::any::type_name;
use std::fmt;
use std::pin::Pin;
use std::ptr::NonNull;

use crate::{LayoutKey, RawBlindPooledMut, RawPooled};

/// A shared handle to an object in a [`RawBlindPool`][crate::RawBlindPool].
/// # Implications of raw handles
///
/// The handle can be used to access the pooled object, as well as to remove
/// it from the pool when no longer needed.
///
/// This is a raw handle that requires manual lifetime management of the pooled objects.
/// * Accessing the target object is only possible via unsafe code as the handle does not
///   know when the pool has been dropped - the caller must guarantee the pool still exists.
/// * You must explicitly remove the target object from the pool when it is no longer needed.
///   If the handle is merely dropped, the object it references remains in the pool until
///   the pool itself is dropped.
///
/// This is a shared handle that only grants shared access to the object. No exclusive
/// references can be created through this handle.
///
/// All handles become invalid once the object is removed from the pool or the pool is dropped.
///
/// # Thread safety
///
/// The handle is always `Sync`. The handle is `Send` if `T` is `Send`.
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

    /// Get a pointer to the target object.
    ///
    /// All pooled objects are guaranteed to be pinned for their entire lifetime, so this pointer
    /// remains valid for as long as the object remains in the pool.
    ///
    /// The object pool implementation does not keep any references to the pooled objects, so
    /// you have the option of using this pointer to create Rust references directly without fear
    /// of any conflicting references created by the pool.
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    pub fn ptr(&self) -> NonNull<T> {
        self.inner.ptr()
    }

    /// Borrows the target object as a pinned shared reference.
    ///
    /// All pooled objects are guaranteed to be pinned for their entire lifetime.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the pool will remain alive for the duration the returned
    /// reference is used.
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    pub unsafe fn as_pin(&self) -> Pin<&T> {
        // SAFETY: Forwarding safety guarantees from the caller.
        let as_ref = unsafe { self.as_ref() };

        // SAFETY: Pooled items are always pinned.
        unsafe { Pin::new_unchecked(as_ref) }
    }

    /// Borrows the target object via shared reference.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the pool remains alive for
    /// the duration the returned reference is used.
    ///
    /// The caller must guarantee that the object remains in the pool
    /// for the duration the returned reference is used.
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
