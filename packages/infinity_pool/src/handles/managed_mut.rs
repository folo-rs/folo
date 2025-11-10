use std::borrow::{Borrow, BorrowMut};
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::Arc;
use std::{fmt, mem, ptr};

use parking_lot::Mutex;

use crate::{Pooled, RawOpaquePoolSend, RawPooledMut};

// Note that while this is a thread-safe handle, we do not require `T: Send` because
// we do not want to require every trait we cast into via trait object to be `Send`.
// It is the responsibility of the pool to ensure that only `Send` objects are inserted.

/// A unique thread-safe reference-counting handle for a pooled object.
#[doc = include_str!("../../doc/snippets/ref_counted_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/unique_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/nonlocal_handle_thread_safety.md")]
pub struct PooledMut<T: ?Sized> {
    inner: RawPooledMut<T>,
    pool: Arc<Mutex<RawOpaquePoolSend>>,
}

impl<T: ?Sized> PooledMut<T> {
    #[must_use]
    pub(crate) fn new(inner: RawPooledMut<T>, pool: Arc<Mutex<RawOpaquePoolSend>>) -> Self {
        Self { inner, pool }
    }

    #[doc = include_str!("../../doc/snippets/handle_ptr.md")]
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    pub fn ptr(&self) -> NonNull<T> {
        self.inner.ptr()
    }

    #[doc = include_str!("../../doc/snippets/handle_into_shared.md")]
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    pub fn into_shared(self) -> Pooled<T> {
        let (inner, pool) = self.into_parts();

        Pooled::new(inner, pool)
    }

    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    fn into_parts(self) -> (RawPooledMut<T>, Arc<Mutex<RawOpaquePoolSend>>) {
        // We transfer these fields to the caller, so we do not want the current handle
        // to be dropped. Hence we perform raw reads to extract the fields directly.

        // SAFETY: The target is valid for reads.
        let pool = unsafe { ptr::read(&raw const self.pool) };
        // SAFETY: The target is valid for reads.
        let inner = unsafe { ptr::read(&raw const self.inner) };

        // We are just "destructuring with Drop" here.
        mem::forget(self);

        (inner, pool)
    }

    #[doc = include_str!("../../doc/snippets/ref_counted_as_pin.md")]
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    pub fn as_pin(&self) -> Pin<&T> {
        // SAFETY: Pooled items are always pinned.
        unsafe { Pin::new_unchecked(self) }
    }

    #[doc = include_str!("../../doc/snippets/ref_counted_as_pin_mut.md")]
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    pub fn as_pin_mut(&mut self) -> Pin<&mut T> {
        // SAFETY: This is a unique handle, so we guarantee borrow safety
        // of the target object by borrowing the handle itself.
        let as_mut = unsafe { self.ptr().as_mut() };

        // SAFETY: Pooled items are always pinned.
        unsafe { Pin::new_unchecked(as_mut) }
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
    pub unsafe fn __private_cast_dyn_with_fn<U: ?Sized, F>(self, cast_fn: F) -> PooledMut<U>
    where
        F: FnOnce(&mut T) -> &mut U,
    {
        let (inner, pool) = self.into_parts();

        // SAFETY: Forwarding callback safety guarantees from the caller.
        // We are an exclusive handle, so we always have the right to create
        // exclusive references to the target of the handle, satisfying that requirement.
        let new_inner = unsafe { inner.__private_cast_dyn_with_fn(cast_fn) };

        PooledMut {
            inner: new_inner,
            pool,
        }
    }
}

impl<T> PooledMut<T>
where
    T: Unpin,
{
    #[doc = include_str!("../../doc/snippets/ref_counted_into_inner.md")]
    #[must_use]
    #[inline]
    pub fn into_inner(self) -> T {
        let (inner, pool) = self.into_parts();

        let mut pool = pool.lock();
        pool.remove_mut_unpin(inner)
    }
}

impl<T: ?Sized> fmt::Debug for PooledMut<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PooledMut")
            .field("inner", &self.inner)
            .field("pool", &self.pool)
            .finish()
    }
}

impl<T: ?Sized> Deref for PooledMut<T> {
    type Target = T;

    #[inline]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants does not understand this signature - every mutation is unviable waste of time.
    fn deref(&self) -> &Self::Target {
        // SAFETY: This is a unique handle, so we guarantee borrow safety
        // of the target object by borrowing the handle itself.
        // We guarantee liveness by being a reference counted handle.
        unsafe { self.ptr().as_ref() }
    }
}

impl<T> DerefMut for PooledMut<T>
where
    T: ?Sized + Unpin,
{
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants does not understand this signature - every mutation is unviable waste of time.
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: This is a unique handle, so we guarantee borrow safety
        // of the target object by borrowing the handle itself.
        // We guarantee liveness by being a reference counted handle.
        unsafe { self.ptr().as_mut() }
    }
}

impl<T: ?Sized> Borrow<T> for PooledMut<T> {
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants does not understand this signature - every mutation is unviable waste of time.
    fn borrow(&self) -> &T {
        self
    }
}

impl<T> BorrowMut<T> for PooledMut<T>
where
    T: ?Sized + Unpin,
{
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants does not understand this signature - every mutation is unviable waste of time.
    fn borrow_mut(&mut self) -> &mut T {
        self
    }
}

impl<T: ?Sized> AsRef<T> for PooledMut<T> {
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants does not understand this signature - every mutation is unviable waste of time.
    fn as_ref(&self) -> &T {
        self
    }
}

impl<T> AsMut<T> for PooledMut<T>
where
    T: ?Sized + Unpin,
{
    #[inline]
    #[cfg_attr(test, mutants::skip)] // Cargo-mutants does not understand this signature - every mutation is unviable waste of time.
    fn as_mut(&mut self) -> &mut T {
        self
    }
}

impl<T: ?Sized> Drop for PooledMut<T> {
    fn drop(&mut self) {
        // While `RawPooledMut` is technically not Copy, we use our insider knowledge
        // that actually it is in reality just a fat pointer, so we can actually copy it.
        // The only reason it is not Copy is to ensure uniqueness, which we do not care
        // about here because the copy in `self` is going away. We just do not want to
        // insert an Option that we have to check in every method.
        //
        // SAFETY: The target is valid for reads.
        let inner = unsafe { ptr::read(&raw const self.inner) };

        let mut pool = self.pool.lock();
        pool.remove_mut(inner);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::thread;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    // u32 is Sync, so PooledMut<u32> should be Send (but not Sync).
    assert_impl_all!(PooledMut<u32>: Send);
    assert_not_impl_any!(PooledMut<u32>: Sync);

    // Cell is Send but not Sync, so PooledMut<Cell> should now be Send (but not Sync)
    // because unique handles only need T: Send.
    assert_impl_all!(PooledMut<Cell<u32>>: Send);
    assert_not_impl_any!(PooledMut<Cell<u32>>: Sync);

    // Non-Send types should make the handle non-Send.
    assert_not_impl_any!(PooledMut<std::rc::Rc<i32>>: Send);

    // We expect no destructor because we treat it as `Copy` in our own Drop::drop().
    assert_not_impl_any!(RawPooledMut<()>: Drop);

    #[test]
    fn unique_handle_can_cross_threads_with_send_only() {
        use crate::OpaquePool;

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

        let pool = OpaquePool::with_layout_of::<Counter>();
        let handle = pool.insert(Counter::new(0));

        // Increment in main thread.
        handle.increment();
        assert_eq!(handle.get(), 1);

        // Move handle to another thread (requires Send but not Sync).
        let handle_in_thread = thread::spawn(move || {
            handle.increment();
            assert_eq!(handle.get(), 2);
            handle
        })
        .join()
        .unwrap();

        // Back in main thread.
        assert_eq!(handle_in_thread.get(), 2);
    }
}
