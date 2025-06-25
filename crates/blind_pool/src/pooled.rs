use std::borrow::Borrow;
use std::ops::Deref;
use std::pin::Pin;
use std::ptr::NonNull;

/// A reference to an item inserted into a [`BlindPool`].
///
/// # Resource management
///
/// You must return this reference to [`BlindPool::remove()`] to remove the item from the pool.
///
/// There is no other way to remove an item from the pool. This type is undroppable - the only
/// way to get rid of an instance is to return it to the pool.
#[derive(Debug)]
pub struct Pooled<T> {
    // There can only ever be one `Pooled<T>` for each item in the pool. The pool itself guarantees
    // that it never creates references to its items, so we know we are the only owner of this item
    // and can freely create references to it, including exclusive references.
    ptr: NonNull<T>,
}

impl<T> Pooled<T> {
    /// Creates a new `Pooled<T>` from a raw pointer.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the pointer is valid for reads and writes of `T` and that there
    /// are no other owners of the item - the `Pooled<T>` must be the only owner, aside from the
    /// pool that owns the storage (but never touches the item itself).
    pub(crate) unsafe fn new(ptr: NonNull<T>) -> Self {
        Pooled { ptr }
    }

    /// Returns a pinned shared reference to the item.
    pub fn get(&self) -> Pin<&T> {
        // SAFETY: See comments on field.
        let as_ref = unsafe { self.ptr.as_ref() };

        // SAFETY: BlindPool guarantees pinning of items.
        unsafe { Pin::new_unchecked(as_ref) }
    }

    /// Returns a pinned exclusive reference to the item.
    pub fn get_mut(&mut self) -> Pin<&mut T> {
        // SAFETY: See comments on field.
        let as_mut = unsafe { self.ptr.as_mut() };

        // SAFETY: BlindPool guarantees pinning of items.
        unsafe { Pin::new_unchecked(as_mut) }
    }
}

impl<T> Deref for Pooled<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        // SAFETY: See comments on field.
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> AsRef<T> for Pooled<T> {
    fn as_ref(&self) -> &T {
        self.deref()
    }
}

impl<T> Borrow<T> for Pooled<T> {
    fn borrow(&self) -> &T {
        self.deref()
    }
}

impl<T> Drop for Pooled<T> {
    fn drop(&mut self) {
        const { panic!("Pooled<T> must be returned to BlindPool::remove() - you cannot drop this type") }
    }
}
