use std::any::type_name;
use std::fmt;
use std::pin::Pin;
use std::ptr::NonNull;

use crate::{RawPooledMut, SlabHandle};

/// A shared handle to an object in an object pool.
#[doc = include_str!("../../doc/snippets/raw_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/shared_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/shared_raw_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/nonlocal_handle_thread_safety.md")]
pub struct RawPooled<T>
where
    // We support casting to trait objects, hence `?Sized`.
    T: ?Sized,
{
    /// Index of the slab in the pool. Slabs are guaranteed to stay at the same index unless
    /// the pool is shrunk (which can only happen when the affected slabs are empty, in which
    /// case all existing handles are already invalidated).
    slab_index: usize,

    /// Handle to the object in the slab. This grants us access to the object's pointer
    /// and allows us to operate on the object (e.g. to remove it or create a reference).
    ///
    /// We inherit the thread-safety properties of the slab handle (Send from T, Sync always).
    slab_handle: SlabHandle<T>,
}

impl<T: ?Sized> RawPooled<T> {
    #[must_use]
    pub(crate) fn new(slab_index: usize, slab_handle: SlabHandle<T>) -> Self {
        Self {
            slab_index,
            slab_handle,
        }
    }

    /// Get the index of the slab in the pool.
    ///
    /// This is used by the pool itself to identify the slab in which the object resides.
    #[must_use]
    pub(crate) fn slab_index(&self) -> usize {
        self.slab_index
    }

    /// Get the slab handle for this pool handle.
    ///
    /// This is used by the pool itself to perform operations on the object in the slab.
    #[must_use]
    pub(crate) fn slab_handle(&self) -> SlabHandle<T> {
        self.slab_handle
    }

    #[doc = include_str!("../../doc/snippets/handle_ptr.md")]
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    pub fn ptr(&self) -> NonNull<T> {
        self.slab_handle.ptr()
    }

    /// Erase the type information from this handle for internal use by remover logic.
    ///
    /// # Safety
    ///
    /// This method is intended for internal use only and does not enforce trait bounds
    /// on the type being erased. The caller must guarantee that the type-erased handle
    /// will not be used in a way that would take advantage of auto traits of `()` that
    /// the original type `T` did not have (such as `Send`).
    ///
    /// For public use, see [`erase()`](Self::erase), which has simplified safety rules
    /// because it can rely on public API constraints being enforced on all accesses.
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    pub(crate) unsafe fn erase_raw(self) -> RawPooled<()> {
        RawPooled {
            slab_index: self.slab_index,
            // SAFETY: Forwarding safety guarantees from the caller.
            slab_handle: unsafe { self.slab_handle.erase() },
        }
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
        // SAFETY: This is a shared handle, so we cannot guarantee borrow safety. What we
        // do instead is that we only allow shared references to be created from shared handles,
        // so the user must explicitly invoke (some other) unsafe code to create an exclusive
        // reference, at which point they become responsible for deconflicting the references.
        // Pointer validity requires pool to be alive for the duration of the reference.
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
    ///
    /// The caller must guarantee that the pool will remain alive for the duration the returned
    /// reference is used.
    #[doc(hidden)]
    #[must_use]
    #[inline]
    pub unsafe fn __private_cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> RawPooled<U>
    where
        F: FnOnce(&T) -> &U,
    {
        // SAFETY: Forwarding callback safety guarantees from the caller.
        // We are a shared handle, so we always have the right to create
        // shared references to the target of the handle, satisfying that requirement.
        let new_handle = unsafe { self.slab_handle.cast_with(cast_fn) };

        RawPooled {
            slab_index: self.slab_index,
            slab_handle: new_handle,
        }
    }

    /// Erase the type information from this handle, converting it to `RawPooled<()>`.
    ///
    /// This is useful for extending the lifetime of an object in the pool without retaining
    /// type information. The type-erased handle prevents access to the object but provides
    /// the capability to type-agnostically remove the object at a later point in time.
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // All mutations unviable - save some time.
    pub fn erase(self) -> RawPooled<()> {
        // SAFETY: The risk is that if `T: !Send` then `Self<()>` would be `Send`. Calling
        // `pool.remove()` with this handle would move the object across threads to drop it.
        // However, this cannot actually happen because for this call to be possible, `pool`
        // would need to at minimum be `Send` (either to move it to a new thread or to wrap in
        // a mutex and gain `Sync` with an exclusive reference), which is only possible if
        // `T: Send` to begin with because that is a condition set by the pool itself.
        unsafe { self.erase_raw() }
    }
}

impl<T: ?Sized> Clone for RawPooled<T> {
    #[inline]
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> Copy for RawPooled<T> {}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: ?Sized> fmt::Debug for RawPooled<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("slab_index", &self.slab_index)
            .field("slab_handle", &self.slab_handle)
            .finish()
    }
}

impl<T: ?Sized> From<RawPooledMut<T>> for RawPooled<T> {
    #[inline]
    fn from(value: RawPooledMut<T>) -> Self {
        value.into_shared()
    }
}

#[cfg(test)]
#[allow(clippy::undocumented_unsafe_blocks, reason = "test code, be concise")]
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::{NotSendNotSync, NotSendSync, SendAndSync, SendNotSync};

    assert_impl_all!(RawPooled<SendAndSync>: Send, Sync);
    assert_impl_all!(RawPooled<SendNotSync>: Send, Sync);
    assert_impl_all!(RawPooled<NotSendNotSync>: Sync);
    assert_impl_all!(RawPooled<NotSendSync>:  Sync);

    assert_not_impl_any!(RawPooled<NotSendNotSync>: Send);
    assert_not_impl_any!(RawPooled<NotSendSync>: Send);

    // Shared raw handles are just fancy pointers, value objects.
    assert_impl_all!(RawPooled<SendAndSync>: Copy);
    assert_not_impl_any!(RawPooled<SendAndSync>: Drop);

    #[test]
    fn erase_raw_and_public_erase() {
        use crate::RawPinnedPool;

        let mut pool = RawPinnedPool::<u32>::new();
        let handle = pool.insert(42);
        let shared = handle.into_shared();

        // Both erase_raw() and erase() produce the same result.
        let erased_raw = unsafe { shared.erase_raw() };
        let erased_public = shared.erase();

        // Both are RawPooled<()> and behave identically.
        assert_eq!(erased_raw.slab_index(), erased_public.slab_index());

        // SAFETY: Handles are valid and pool is alive.
        unsafe {
            pool.remove(erased_raw);
        }

        assert_eq!(pool.len(), 0);
    }
}
