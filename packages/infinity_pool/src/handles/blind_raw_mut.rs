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
    inner: RawPooledMut<T>,
}

impl<T: ?Sized> RawBlindPooledMut<T> {
    #[must_use]
    pub(crate) fn new(key: LayoutKey, inner: RawPooledMut<T>) -> Self {
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
    pub(crate) fn into_inner(self) -> RawPooledMut<T> {
        self.inner
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
}

impl<T: ?Sized + Unpin> RawBlindPooledMut<T> {
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

impl<T: ?Sized> fmt::Debug for RawBlindPooledMut<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RawBlindPooledMut")
            .field("key", &self.key)
            .field("inner", &self.inner)
            .finish()
    }
}

// SAFETY: RawBlindPooledMut provides unique access to T. When the handle moves between
// threads, T moves with it atomically. No concurrent access is possible through
// a unique handle, so we only require T: Send, not T: Sync.
unsafe impl<T: ?Sized + Send> Send for RawBlindPooledMut<T> {}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::thread;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    // u32 is Sync, so RawBlindPooledMut<u32> should be Send (but not Sync).
    assert_impl_all!(RawBlindPooledMut<u32>: Send);
    assert_not_impl_any!(RawBlindPooledMut<u32>: Sync);

    // Cell is Send but not Sync, so RawBlindPooledMut<Cell> should now be Send (but not Sync)
    // because unique handles only need T: Send.
    assert_impl_all!(RawBlindPooledMut<Cell<u32>>: Send);
    assert_not_impl_any!(RawBlindPooledMut<Cell<u32>>: Sync);

    // Non-Send types should make the handle non-Send.
    assert_not_impl_any!(RawBlindPooledMut<std::rc::Rc<i32>>: Send);

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
