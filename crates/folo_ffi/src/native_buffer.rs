#![allow(dead_code)] // Probably will use later.

use std::{
    alloc::Layout,
    marker::PhantomData,
    mem::{self, MaybeUninit},
    num::NonZeroUsize,
    ops::Range,
    ptr::NonNull,
};

/// Windows often wants us to provide a buffer of arbitrary size aligned for a type `T`, in which
/// it wants to place instances of `T` or `T`-like types (not necessarily at integer `T` offsets).
///
/// This is a wrapper around alloc/dealloc to make working with such buffers slightly convenient.
pub struct NativeBuffer<T: Sized> {
    ptr: NonNull<u8>,
    layout: Layout,

    // How many bytes of data have been used from the buffer's (layout-defined) capacity.
    len: usize,

    _phantom: PhantomData<T>,
}

impl<T: Sized> NativeBuffer<T> {
    pub fn new(size_bytes: NonZeroUsize) -> Self {
        // Sometimes Windows will ask for less memory than technically required to fit a T.
        // While this may be valid as long as Rust never tries to read that extra memory (e.g.
        // because the larger union members are not used), it is still feels risky, so
        // we round up to the minimum size required to fit one T to avoid any accidents.
        // In cases where we have multiple T in the buffer, the risk of "reading off the edge"
        // (albeit requiring invalid code) still remains and needs to be guarded against upstack.
        let min_size = mem::size_of::<T>();

        let size_bytes = if size_bytes.get() < min_size {
            NonZeroUsize::new(min_size).expect("zero-sized T is not supported")
        } else {
            size_bytes
        };

        let layout = Layout::from_size_align(size_bytes.get(), mem::align_of::<T>())
            .expect("called with arguments that yielded an invalid memory layout");

        // SAFETY: We must promise to provide a layout of nonzero size. All is well.
        let ptr = unsafe { std::alloc::alloc(layout) };
        let ptr =
            NonNull::new(ptr).expect("allocation failed - the app cannot continue to operate");

        Self {
            ptr,
            layout,
            len: 0,
            _phantom: PhantomData,
        }
    }

    /// Writes a value at the start of the buffer and declares the buffer length to match.
    pub fn emplace(&mut self, value: T) {
        // SAFETY: Type invariants guarantee the pointer is properly aligned
        // and the borrow checker ensures it is valid for writes via `self`.
        unsafe {
            self.ptr.cast::<T>().write(value);
        }

        // SAFETY: We just wrote a T into it, so obviously there is a T worth of data in there.
        unsafe {
            self.set_len(mem::size_of::<T>());
        }
    }

    /// Allocates a buffer that can exactly fit a `T` and moves the `T` into it.
    pub fn from_value(value: T) -> Self {
        let mut buffer = Self::new(
            NonZeroUsize::new(mem::size_of::<T>())
                .expect("cannot create NativeBuffer from zero-sized type"),
        );
        buffer.emplace(value);

        // SAFETY: We just wrote a T into it, so obviously there is a T worth of data in there.
        unsafe {
            buffer.set_len(mem::size_of::<T>());
        }

        buffer
    }

    /// Allocates a buffer that can fit a number of consecutive instances of `T` and moves the
    /// provided instances into the buffer.
    pub fn from_items(items: impl IntoIterator<Item = T>) -> Self {
        let items = items.into_iter().collect::<Vec<_>>();

        let mut buffer = Self::new(
            NonZeroUsize::new(mem::size_of::<T>() * items.len())
                .expect("cannot create NativeBuffer from zero-sized type"),
        );

        let count = items.len();

        let mut dest = buffer.ptr.cast::<T>();
        for item in items {
            // SAFETY: We own the memory and type invariants guarantee proper alignment,
            // so this pointer is safe to write through.
            unsafe {
                dest.write(item);
            }

            // SAFETY: Rust language guarantees that array layout is compatible with this model.
            dest = unsafe { dest.add(1) };
        }

        // SAFETY: We just wrote N*T bytes into it, math guaranteed valid by Rust language rules.
        unsafe {
            buffer.set_len(mem::size_of::<T>() * count);
        }

        buffer
    }

    /// Capacity of the buffer, in bytes.
    pub fn capacity(&self) -> usize {
        self.layout.size()
    }

    /// Length of the declared data in the buffer, in bytes.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the buffer is considered empty.
    ///
    /// This is determined by `set_len()` being called with a non-zero value.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Declares the length of the data in the buffer, in bytes.
    ///
    /// # Safety
    ///
    /// The caller guarantees that the indicated number of bytes have been initialized.
    pub unsafe fn set_len(&mut self, len: usize) {
        assert!(len <= self.capacity());

        self.len = len;
    }

    /// Returns a pointer to the start of the buffer.
    pub fn as_ptr(&self) -> NonNull<T> {
        self.ptr.cast()
    }

    /// Returns a range of pointers from the start of the buffer to just past the end of the buffer.
    pub fn as_ptr_range(&self) -> Range<*const T> {
        let start = self.ptr.as_ptr().cast_const().cast();

        // SAFETY: We have allocated `self.len` bytes of data in the buffer so the add is valid.
        // It is also valid for pointers to point just past the end of an allocation.
        let end = unsafe { self.ptr.as_ptr().cast_const().byte_add(self.len).cast() };

        start..end
    }
}

impl<T: Sized> AsMut<MaybeUninit<T>> for NativeBuffer<T> {
    fn as_mut(&mut self) -> &mut MaybeUninit<T> {
        // SAFETY: We are returning MaybeUninit, so it is safe to reference the contents.
        // Borrowing rules are enforced via the borrow on `self`, which is extended here.
        unsafe { self.ptr.cast().as_mut() }
    }
}

impl<T: Sized> AsRef<MaybeUninit<T>> for NativeBuffer<T> {
    fn as_ref(&self) -> &MaybeUninit<T> {
        // SAFETY: We are returning MaybeUninit, so it is safe to reference the contents.
        // Borrowing rules are enforced via the borrow on `self`, which is extended here.
        unsafe { self.ptr.cast().as_ref() }
    }
}

impl<T: Sized> Drop for NativeBuffer<T> {
    #[cfg_attr(test, mutants::skip)] // Impractical to test that dealloc actually happens.
    fn drop(&mut self) {
        // SAFETY: We are required to provide a matching layout, the same as we used for alloc().
        // We do that. All is well.
        unsafe {
            std::alloc::dealloc(self.ptr.as_ptr(), self.layout);
        }
    }
}
