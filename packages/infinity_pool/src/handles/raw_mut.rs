use std::any::type_name;
use std::fmt;
use std::pin::Pin;
use std::ptr::NonNull;

use crate::{RawPooled, SlabHandle};

/// A unique handle to an object in an object pool.
#[doc = include_str!("../../doc/snippets/raw_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/unique_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/unique_raw_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/nonlocal_handle_thread_safety.md")]
pub struct RawPooledMut<T>
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

impl<T: ?Sized> RawPooledMut<T> {
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

    #[doc = include_str!("../../doc/snippets/handle_into_shared.md")]
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    pub fn into_shared(self) -> RawPooled<T> {
        RawPooled::new(self.slab_index, self.slab_handle)
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

    #[doc = include_str!("../../doc/snippets/raw_as_pin_mut.md")]
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    pub unsafe fn as_pin_mut(&mut self) -> Pin<&mut T> {
        // SAFETY: This is a unique handle, so we guarantee borrow safety
        // of the target object by borrowing the handle itself. Pointer validity
        // requires pool to be alive, which is a safety requirement of this function.
        let as_mut = unsafe { self.ptr().as_mut() };

        // SAFETY: Pooled items are always pinned.
        unsafe { Pin::new_unchecked(as_mut) }
    }

    #[doc = include_str!("../../doc/snippets/raw_mut_as_ref.md")]
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    pub unsafe fn as_ref(&self) -> &T {
        // SAFETY: This is a unique handle, so we guarantee borrow safety
        // of the target object by borrowing the handle itself. Pointer validity
        // requires pool to be alive, which is a safety requirement of this function.
        unsafe { self.ptr().as_ref() }
    }

    /// Erase the type information from this handle, converting it to `RawPooledMut<()>`.
    ///
    /// This is useful for extending the lifetime of an object in the pool without retaining
    /// type information. The type-erased handle prevents access to the object but ensures
    /// it remains in the pool.
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // All mutations unviable - save some time.
    pub fn erase(self) -> RawPooledMut<()> {
        RawPooledMut {
            slab_index: self.slab_index,
            // SAFETY: The risk is that if `T: !Send` then `Self<()>` would be `Send`. Calling
            // `pool.remove()` with this handle would move the object across threads to drop it.
            // However, this cannot actually happen because for this call to be possible, `pool`
            // would need to at minimum be `Send` (either to move it to a new thread or to wrap in
            // a mutex and gain `Sync` with an exclusive reference), which is only possible if
            // `T: Send` to begin with because that is a condition set by the pool itself.
            slab_handle: unsafe { self.slab_handle.erase() },
        }
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
    pub unsafe fn __private_cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> RawPooledMut<U>
    where
        F: FnOnce(&mut T) -> &mut U,
    {
        // SAFETY: Forwarding callback safety guarantees from the caller.
        // We are an exclusive handle, so we always have the right to create
        // exclusive references to the target of the handle, satisfying that requirement.
        let new_handle = unsafe { self.slab_handle.cast_with_mut(cast_fn) };

        RawPooledMut {
            slab_index: self.slab_index,
            slab_handle: new_handle,
        }
    }
}

impl<T: ?Sized + Unpin> RawPooledMut<T> {
    #[doc = include_str!("../../doc/snippets/raw_as_mut.md")]
    #[must_use]
    #[inline]
    pub unsafe fn as_mut(&mut self) -> &mut T {
        // SAFETY: This is a unique handle, so we guarantee borrow safety
        // of the target object by borrowing the handle itself. Pointer validity
        // requires pool to be alive, which is a safety requirement of this function.
        unsafe { self.ptr().as_mut() }
    }
}

impl<T: ?Sized> fmt::Debug for RawPooledMut<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("slab_index", &self.slab_index)
            .field("slab_handle", &self.slab_handle)
            .finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::Cell;
    use std::thread;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;
    use crate::{NotSendNotSync, NotSendSync, RawPinnedPool, SendAndSync, SendNotSync};

    assert_impl_all!(RawPooledMut<SendAndSync>: Send, Sync);
    assert_impl_all!(RawPooledMut<SendNotSync>: Send, Sync);
    assert_impl_all!(RawPooledMut<NotSendNotSync>: Sync);
    assert_impl_all!(RawPooledMut<NotSendSync>: Sync);

    assert_not_impl_any!(RawPooledMut<NotSendNotSync>: Send);
    assert_not_impl_any!(RawPooledMut<NotSendSync>: Send);

    // This is a unique handle, it cannot be cloneable/copyable.
    assert_not_impl_any!(RawPooledMut<SendAndSync>: Clone, Copy);

    // This is not strictly a requirement but a destructor is not something we expect here.
    assert_not_impl_any!(RawPooledMut<SendAndSync>: Drop);

    #[test]
    fn unique_handle_can_cross_threads_with_send_only() {
        // A type that is Send but not Sync.
        struct Counter {
            value: Cell<i32>,
        }

        // SAFETY: Counter is designed to be Send but not Sync for testing.
        unsafe impl Send for Counter {}

        impl Counter {
            fn new(value: i32) -> Self {
                Self {
                    value: Cell::new(value),
                }
            }

            fn increment(&self) {
                self.value.set(self.value.get() + 1);
            }

            fn get(&self) -> i32 {
                self.value.get()
            }
        }

        let mut pool = RawPinnedPool::<Counter>::new();
        let handle = pool.insert(Counter::new(0));

        // Increment in main thread.
        // SAFETY: Handle is valid and pool is still alive.
        unsafe { handle.ptr().as_ref() }.increment();
        // SAFETY: Handle is valid and pool is still alive.
        assert_eq!(unsafe { handle.ptr().as_ref() }.get(), 1);

        // Move handle to another thread (requires Send but not Sync).
        let handle_in_thread = thread::spawn(move || {
            // SAFETY: Handle is valid and pool is still alive.
            unsafe { handle.ptr().as_ref() }.increment();
            // SAFETY: Handle is valid and pool is still alive.
            assert_eq!(unsafe { handle.ptr().as_ref() }.get(), 2);
            handle
        })
        .join()
        .unwrap();

        // Back in main thread.
        // SAFETY: Handle is valid and pool is still alive.
        assert_eq!(unsafe { handle_in_thread.ptr().as_ref() }.get(), 2);
    }

    #[cfg_attr(miri, ignore)] // Too much data for Miri - runs too slow.
    #[test]
    fn slab_index_returns_correct_value() {
        let mut pool = RawPinnedPool::<u64>::new();

        // Insert first item - should be in slab 0.
        let handle1 = pool.insert(42);
        assert_eq!(handle1.slab_index(), 0);

        // Insert more items - should all be in slab 0 until it fills up.
        let handle2 = pool.insert(100);
        assert_eq!(handle2.slab_index(), 0);

        // Create enough items to force allocation of a new slab.
        // Default slab capacity varies, so we insert many items.
        let mut handles = Vec::new();
        for i in 0..10000 {
            handles.push(pool.insert(i));
        }

        // At least some handles should be in slab 1 or higher.
        let has_slab_1_or_higher = handles.iter().any(|h| h.slab_index() > 0);
        assert!(has_slab_1_or_higher);

        // All handles should have valid slab indices.
        for handle in &handles {
            assert!(handle.slab_index() < pool.capacity());
        }
    }
}
