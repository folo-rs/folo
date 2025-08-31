use std::ptr::{self, NonNull};

/// A handle to an object stored in a `Slab`.
///
/// This can be used to obtain a typed pointer to the inserted item, as well as to remove
/// the item from the slab when no longer needed.
///
/// The handle enforces no ownership semantics - consider it merely a fat pointer. It can be
/// freely cloned and copied. The only way to access the contents is unsafe access through
/// the pointer obtained via `ptr()`.
///
/// # Thread safety
///
/// The handle provides access to the underlying object, so its thread-safety characteristics
/// are determined by the type of the object it points to.
///
/// If the underlying object is `Sync`, the handle is thread-mobile (`Send`). Otherwise, the
/// handle is single-threaded (neither `Send` nor `Sync`).
#[derive(Debug)]
pub(crate) struct SlabHandle<T: ?Sized> {
    /// Index in the slab at which this item is stored.
    ///
    /// This is used for removal of the object.
    ///
    /// Note that slab indexes may be reused after an object is removed. It is the responsibility
    /// of the caller to not use a handler after the object has been removed (this is generally
    /// going to be a safety requirement of any method that returns `SlabHandle`).
    index: usize,

    /// The object this handle points to in the slab.
    ///
    /// Note that slab slots may be reused after an object is removed. It is the responsibility
    /// of the caller to not use a handler after the object has been removed (this is generally
    /// going to be a safety requirement of any method that returns `SlabHandle`).
    ptr: NonNull<T>,
}

impl<T: ?Sized> Clone for SlabHandle<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> Copy for SlabHandle<T> {}

impl<T: ?Sized> PartialEq for SlabHandle<T> {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index && ptr::addr_eq(self.ptr.as_ptr(), other.ptr.as_ptr())
    }
}

impl<T: ?Sized> Eq for SlabHandle<T> {}

impl<T: ?Sized> SlabHandle<T> {
    pub(crate) fn new(index: usize, ptr: NonNull<T>) -> Self {
        Self { index, ptr }
    }

    /// Get the index of the object in the slab.
    ///
    /// This is used by the slab itself to identify the slot to be freed upon removal.
    pub(crate) fn index(&self) -> usize {
        self.index
    }

    /// Get a raw pointer to the object in the slab.
    ///
    /// It is the responsibility of the caller to ensure that the pointer is not used
    /// after the object has been removed from the slab.
    pub(crate) fn ptr(&self) -> NonNull<T> {
        self.ptr
    }

    /// Erases the type of the object the slab handle points to.
    ///
    /// The returned handle remains functional for most purposes, just without type information.
    /// A type-erased handle cannot be used to remove the object from the slab and return it to
    /// the caller, as there is no more knowledge of the type to be returned.
    pub(crate) fn erase(self) -> SlabHandle<()> {
        SlabHandle {
            index: self.index,
            ptr: self.ptr.cast(),
        }
    }

    // TODO: cast to trait object
}

// SAFETY: See type-level documentation.
unsafe impl<T: ?Sized + Sync> Send for SlabHandle<T> {}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    // u32 is Sync, so SlabHandle<u32> should be Send (but not Sync).
    assert_impl_all!(SlabHandle<u32>: Send);
    assert_not_impl_any!(SlabHandle<u32>: Sync);

    // Cell is Send but not Sync, so SlabHandle<Cell> should be neither Send nor Sync.
    assert_not_impl_any!(SlabHandle<Cell<u32>>: Send, Sync);

    #[test]
    fn same_index_different_ptr_is_not_equal() {
        let mut x = 42_u32;
        let mut y = 42_u32;
        let handle1 = SlabHandle::new(0, NonNull::from(&mut x));
        let handle2 = SlabHandle::new(0, NonNull::from(&mut y));
        assert_ne!(handle1, handle2);
    }

    #[test]
    fn same_index_same_ptr_is_equal() {
        let mut x = 42_u32;
        let ptr = NonNull::from(&mut x);
        let handle1 = SlabHandle::new(0, ptr);
        let handle2 = SlabHandle::new(0, ptr);
        assert_eq!(handle1, handle2);
    }

    #[test]
    fn different_index_same_ptr_is_not_equal() {
        let mut x = 42_u32;
        let ptr = NonNull::from(&mut x);
        let handle1 = SlabHandle::new(0, ptr);
        let handle2 = SlabHandle::new(1, ptr);
        assert_ne!(handle1, handle2);
    }

    #[test]
    fn clone_copy_are_equal() {
        let mut x = 42_u32;
        let handle = SlabHandle::new(5, NonNull::from(&mut x));
        let cloned = Clone::clone(&handle);
        let copied = handle;
        assert_eq!(handle, cloned);
        assert_eq!(handle, copied);
        assert_eq!(cloned, copied);
    }

    #[test]
    fn erase_preserves_equality() {
        let mut x = 42_u32;
        let mut y = 42_u32;
        let handle1 = SlabHandle::new(5, NonNull::from(&mut x));
        let handle2 = SlabHandle::new(5, NonNull::from(&mut x));
        let handle3 = SlabHandle::new(5, NonNull::from(&mut y));

        // Handles pointing to same location should be equal before and after erase
        assert_eq!(handle1, handle2);
        assert_eq!(handle1.erase(), handle2.erase());

        // Handles pointing to different locations should be unequal before and after erase
        assert_ne!(handle1, handle3);
        assert_ne!(handle1.erase(), handle3.erase());

        // The erased handle should maintain the same index
        let erased = handle1.erase();
        assert_eq!(erased.index(), handle1.index());
    }
}
