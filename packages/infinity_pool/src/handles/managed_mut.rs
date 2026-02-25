use std::any::type_name;
use std::borrow::{Borrow, BorrowMut};
use std::ops::{Deref, DerefMut};
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::Arc;
use std::{fmt, mem, ptr};

use parking_lot::Mutex;

use crate::{Pooled, RawOpaquePoolThreadSafe, RawPooledMut};

// Note that while this is a thread-safe handle, we do not require `T: Send` because
// we do not want to require every trait we cast into via trait object to be `Send`.
// It is the responsibility of the pool to ensure that only `Send` objects are inserted.

/// A unique thread-safe reference-counting handle for a pooled object.
#[doc = include_str!("../../doc/snippets/ref_counted_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/unique_handle_implications.md")]
#[doc = include_str!("../../doc/snippets/nonlocal_handle_thread_safety.md")]
pub struct PooledMut<T: ?Sized> {
    // We inherit our thread-safety traits from this one (Send from T, Sync always).
    inner: RawPooledMut<T>,

    pool: Arc<Mutex<RawOpaquePoolThreadSafe>>,
}

impl<T: ?Sized> PooledMut<T> {
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

        // SAFETY: Guaranteed by ::new() - we only ever create these handles for `T: Send` but
        // we may just lose the `Send` trait from the signature if we cast or erase the type.
        unsafe { Pooled::new(inner, pool) }
    }

    #[cfg_attr(test, mutants::skip)] // cargo-mutants tries many unviable mutations, wasting precious build minutes.
    fn into_parts(self) -> (RawPooledMut<T>, Arc<Mutex<RawOpaquePoolThreadSafe>>) {
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

    /// Erase the type information from this handle, converting it to `PooledMut<()>`.
    ///
    /// This is useful for extending the lifetime of an object in the pool without retaining
    /// type information. The type-erased handle prevents access to the object but ensures
    /// it remains in the pool.
    #[must_use]
    #[inline]
    #[cfg_attr(test, mutants::skip)] // All mutations unviable - save some time.
    pub fn erase(self) -> PooledMut<()> {
        let (inner, pool) = self.into_parts();

        PooledMut {
            inner: inner.erase(),
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
        // SAFETY: We are a managed unique handle, so we are the only one who is allowed to remove
        // the object from the pool - as long as we exist, the object exists in the pool. We keep
        // the pool alive for as long as any handle to it exists, so the pool must still exist.
        unsafe { pool.remove_unpin(inner) }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: ?Sized> fmt::Debug for PooledMut<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
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

        // SAFETY: We are a managed unique handle, so we are the only one who is allowed to remove
        // the object from the pool - as long as we exist, the object exists in the pool. We keep
        // the pool alive for as long as any handle to it exists, so the pool must still exist.
        unsafe {
            pool.remove(inner);
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::Cell;
    use std::thread;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use std::borrow::Borrow;
    use std::borrow::BorrowMut;

    use super::*;
    use crate::{NotSendNotSync, NotSendSync, OpaquePool, SendAndSync, SendNotSync};

    assert_impl_all!(PooledMut<SendAndSync>: Send, Sync);
    assert_impl_all!(PooledMut<SendNotSync>: Send, Sync);
    assert_impl_all!(PooledMut<NotSendNotSync>: Sync);
    assert_impl_all!(PooledMut<NotSendSync>: Sync);

    assert_not_impl_any!(PooledMut<NotSendNotSync>: Send);
    assert_not_impl_any!(PooledMut<NotSendSync>: Send);

    // This is a unique handle, it cannot be cloneable/copyable.
    assert_not_impl_any!(PooledMut<SendAndSync>: Clone, Copy);

    // We must have a destructor because we need to remove the object on destroy.
    assert_impl_all!(PooledMut<SendAndSync>: Drop);

    // We expect no destructor because we treat it as `Copy` in our own Drop::drop().
    assert_not_impl_any!(RawPooledMut<()>: Drop);

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

    #[test]
    fn erase_extends_lifetime() {
        let pool = OpaquePool::with_layout_of::<u32>();
        let handle = pool.insert(42);

        // Erase the unique handle.
        let erased = handle.erase();

        // Object still alive in erased handle.
        assert_eq!(pool.len(), 1);

        // Drop erased handle, object is removed.
        drop(erased);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn erase_then_convert_to_shared() {
        let pool = OpaquePool::with_layout_of::<String>();
        let handle = pool.insert(String::from("test"));

        // Erase and convert to shared.
        let erased_mut = handle.erase();
        let erased_shared = erased_mut.into_shared();
        let erased_clone = erased_shared.clone();

        assert_eq!(pool.len(), 1);

        drop(erased_shared);
        assert_eq!(pool.len(), 1);

        drop(erased_clone);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn as_pin_returns_pinned_reference() {
        let pool = OpaquePool::with_layout_of::<u32>();
        let handle = pool.insert(42_u32);

        let pinned = handle.as_pin();
        assert_eq!(*pinned.get_ref(), 42);
    }

    #[test]
    fn as_pin_mut_returns_pinned_mutable_reference() {
        let pool = OpaquePool::with_layout_of::<u32>();
        let mut handle = pool.insert(42_u32);

        *handle.as_pin_mut().get_mut() = 99;
        assert_eq!(*handle, 99);
    }

    #[test]
    fn deref_returns_reference_to_value() {
        let pool = OpaquePool::with_layout_of::<u32>();
        let handle = pool.insert(42_u32);

        assert_eq!(*handle, 42);
    }

    #[test]
    fn deref_mut_allows_mutation() {
        let pool = OpaquePool::with_layout_of::<u32>();
        let mut handle = pool.insert(42_u32);

        *handle = 99;
        assert_eq!(*handle, 99);
    }

    #[test]
    fn borrow_returns_reference() {
        let pool = OpaquePool::with_layout_of::<u32>();
        let handle = pool.insert(42_u32);

        let borrowed: &u32 = handle.borrow();
        assert_eq!(*borrowed, 42);
    }

    #[test]
    fn borrow_mut_returns_mutable_reference() {
        let pool = OpaquePool::with_layout_of::<u32>();
        let mut handle = pool.insert(42_u32);

        let borrowed: &mut u32 = handle.borrow_mut();
        *borrowed = 99;
        assert_eq!(*handle, 99);
    }

    #[test]
    fn as_ref_returns_reference() {
        let pool = OpaquePool::with_layout_of::<u32>();
        let handle = pool.insert(42_u32);

        let reference: &u32 = handle.as_ref();
        assert_eq!(*reference, 42);
    }

    #[test]
    fn as_mut_returns_mutable_reference() {
        let pool = OpaquePool::with_layout_of::<u32>();
        let mut handle = pool.insert(42_u32);

        let reference: &mut u32 = handle.as_mut();
        *reference = 99;
        assert_eq!(*handle, 99);
    }

    #[test]
    fn into_inner_returns_value() {
        let pool = OpaquePool::with_layout_of::<u32>();
        let handle = pool.insert(42_u32);

        let value = handle.into_inner();
        assert_eq!(value, 42);
        assert_eq!(pool.len(), 0);
    }

    #[test]
    fn into_shared_converts_to_shared_handle() {
        let pool = OpaquePool::with_layout_of::<u32>();
        let handle = pool.insert(42_u32);

        let shared = handle.into_shared();
        assert_eq!(*shared, 42);
        assert_eq!(pool.len(), 1);
    }

    #[test]
    fn from_converts_mut_to_shared() {
        let pool = OpaquePool::with_layout_of::<u32>();
        let handle = pool.insert(42_u32);

        let shared: Pooled<u32> = Pooled::from(handle);
        assert_eq!(*shared, 42);
        assert_eq!(pool.len(), 1);
    }
}
