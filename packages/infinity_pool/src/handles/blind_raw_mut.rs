use std::any::type_name;
use std::fmt;
use std::pin::Pin;
use std::ptr::NonNull;

use crate::{LayoutKey, RawBlindPooled, RawPooledMut};

/// A unique handle to an object in a [`RawBlindPool`][crate::RawBlindPool].
#[doc = include_str!("../../doc/snippets/raw_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/unique_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/unique_raw_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/nonlocal_handle_thread_safety.md")]
pub struct RawBlindPooledMut<T>
where
    // We support casting to trait objects, hence `?Sized`.
    T: ?Sized,
{
    key: LayoutKey,

    // We inherit our thread-safety traits from this one (Send from T, Sync always).
    inner: RawPooledMut<T>,
}

impl<T: ?Sized> RawBlindPooledMut<T> {
    #[must_use]
    pub(crate) fn new(key: LayoutKey, inner: RawPooledMut<T>) -> Self {
        Self { key, inner }
    }

    #[doc = include_str!("../../doc/snippets/handle_ptr.md")]
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    pub fn ptr(&self) -> NonNull<T> {
        self.inner.ptr()
    }

    /// Transforms the unique handle into a shared handle that can be cloned and copied freely.
    ///
    /// A shared handle does not support the creation of exclusive references to the target object
    /// and requires the caller to guarantee that no further access is attempted through any handle
    /// after removing the object from the pool.
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    pub fn into_shared(self) -> RawBlindPooled<T> {
        RawBlindPooled::new(self.key, self.inner.into_shared())
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
    pub unsafe fn __private_cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> RawBlindPooledMut<U>
    where
        F: FnOnce(&mut T) -> &mut U,
    {
        // SAFETY: Forwarding callback safety guarantees from the caller.
        // We are an exclusive handle, so we always have the right to create
        // exclusive references to the target of the handle, satisfying that requirement.
        let new_inner = unsafe { self.inner.__private_cast_dyn_with_fn(cast_fn) };

        RawBlindPooledMut {
            key: self.key,
            inner: new_inner,
        }
    }

    /// Erase the type information from this handle, converting it to `RawBlindPooledMut<()>`.
    ///
    /// This is useful for extending the lifetime of an object in the pool without retaining
    /// type information. The type-erased handle prevents access to the object but ensures
    /// it remains in the pool.
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // All mutations unviable - save some time.
    pub fn erase(self) -> RawBlindPooledMut<()>
    where
        T: Send,
    {
        RawBlindPooledMut {
            key: self.key,
            // This is safe only because `RawBlindPool` is always `!Send`, requiring unsafe code
            // to send across threads (which implies the user manually guaranteeing only `T: Send`
            // are placed into the pool). This is a behavior that the inner handle relies upon.
            // It assumes (correctly) that the pool it is associated with cannot be sent to another
            // thread (via safe code) unless `T: Send`. In our case, we just blanket-disable it
            // for soundness, as we cannot programmatically know whether everything in the pool
            // is `Send` or not.
            inner: self.inner.erase(),
        }
    }
}

impl<T: ?Sized + Unpin> RawBlindPooledMut<T> {
    #[doc = include_str!("../../doc/snippets/raw_as_mut.md")]
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants does not understand this signature - every mutation is unviable waste of time.
    pub unsafe fn as_mut(&mut self) -> &mut T {
        // SAFETY: This is a unique handle, so we guarantee borrow safety
        // of the target object by borrowing the handle itself. Pointer validity
        // requires pool to be alive, which is a safety requirement of this function.
        unsafe { self.ptr().as_mut() }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: ?Sized> fmt::Debug for RawBlindPooledMut<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("key", &self.key)
            .field("inner", &self.inner)
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
    use crate::{NotSendNotSync, NotSendSync, SendAndSync, SendNotSync};

    assert_impl_all!(RawBlindPooledMut<SendAndSync>: Send, Sync);
    assert_impl_all!(RawBlindPooledMut<SendNotSync>: Send, Sync);
    assert_impl_all!(RawBlindPooledMut<NotSendNotSync>: Sync);
    assert_impl_all!(RawBlindPooledMut<NotSendSync>: Sync);

    assert_not_impl_any!(RawBlindPooledMut<NotSendNotSync>: Send);
    assert_not_impl_any!(RawBlindPooledMut<NotSendSync>: Send);

    // This is a unique handle, it cannot be cloneable/copyable.
    assert_not_impl_any!(RawBlindPooledMut<SendAndSync>: Clone, Copy);

    // This is not strictly a requirement but a destructor is not something we expect here.
    assert_not_impl_any!(RawBlindPooledMut<SendAndSync>: Drop);

    #[test]
    fn unique_handle_can_cross_threads_with_send_only() {
        use crate::RawBlindPool;

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

        let mut pool = RawBlindPool::new();
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
}
