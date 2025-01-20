use std::{
    alloc::Layout,
    marker::PhantomData,
    mem::{self, MaybeUninit},
    num::NonZeroUsize,
    ptr::NonNull,
};

/// Windows often wants us to provide a buffer of arbitrary size aligned for a type `T`.
///
/// This is a wrapper around alloc/dealloc to make working with such buffers slightly convenient.
/// It is still a major hassle but one step at a time.
pub(super) struct NativeBuffer<T: Sized> {
    ptr: NonNull<u8>,
    layout: Layout,

    _phantom: PhantomData<T>,
}

impl<T: Sized> NativeBuffer<T> {
    pub fn new(size_bytes: NonZeroUsize) -> Self {
        let min_size = mem::size_of::<T>();

        assert!(size_bytes.get() >= min_size);

        let layout = Layout::from_size_align(size_bytes.get(), mem::align_of::<T>())
            .expect("called with arguments that yielded an invalid memory layout");

        // SAFETY: We must promise to provide a layout of nonzero size. All is well.
        let ptr = unsafe { std::alloc::alloc(layout) };
        let ptr =
            NonNull::new(ptr).expect("allocation failed - the app cannot continue to operate");

        Self {
            ptr,
            layout,
            _phantom: PhantomData,
        }
    }

    /// Writes a value at the start of the buffer.
    pub fn emplace(&mut self, value: T) {
        // SAFETY: Type invariants guarantee the pointer is properly aligned
        // and the borrow checker ensures it is valid for writes via `self`.
        unsafe {
            self.ptr.cast::<T>().write(value);
        }
    }

    /// Allocates a buffer that can exactly fit a `T` and moves the `T` into it.
    pub fn from_value(value: T) -> Self {
        let mut buffer = Self::new(
            NonZeroUsize::new(mem::size_of::<T>())
                .expect("cannot create NativeBuffer from zero-sized type"),
        );
        buffer.emplace(value);
        buffer
    }

    pub fn from_vec(vec: Vec<T>) -> Self {
        let buffer = Self::new(
            NonZeroUsize::new(mem::size_of::<T>() * vec.len())
                .expect("cannot create NativeBuffer from zero-sized type"),
        );

        let mut dest = buffer.ptr.cast::<T>();
        for item in vec {
            // SAFETY: We own the memory and type invariants guarantee proper alignment,
            // so this pointer is safe to write through.
            unsafe {
                dest.write(item);
            }

            // SAFETY: Rust language guarantees that array layout is compatible with this model.
            dest = unsafe { dest.add(1) };
        }

        buffer
    }

    /// Length of the buffer in bytes.
    pub fn len(&self) -> usize {
        self.layout.size()
    }

    pub fn as_ptr(&self) -> NonNull<T> {
        self.ptr.cast()
    }

    pub fn as_mut(&mut self) -> &mut MaybeUninit<T> {
        // SAFETY: We are returning MaybeUninit, so it is safe to reference the contents.
        // Borrowing rules are enforced via the borrow on `self`, which is extended here.
        unsafe { self.ptr.cast().as_mut() }
    }

    pub fn as_ref(&self) -> &MaybeUninit<T> {
        // SAFETY: We are returning MaybeUninit, so it is safe to reference the contents.
        // Borrowing rules are enforced via the borrow on `self`, which is extended here.
        unsafe { self.ptr.cast().as_ref() }
    }
}

impl<T: Sized> Drop for NativeBuffer<T> {
    fn drop(&mut self) {
        // SAFETY: We are required to provide a matching layout, the same as we used for alloc().
        // We do that. All is well.
        unsafe {
            std::alloc::dealloc(self.ptr.as_ptr(), self.layout);
        }
    }
}
