use std::alloc::Layout;
use std::marker::PhantomData;
use std::mem::MaybeUninit;
use std::num::NonZero;
use std::ops::Range;
use std::ptr::NonNull;

/// Windows often wants us to provide a buffer of arbitrary size aligned for a type `T`, in which
/// it wants to place instances of `T` or `T`-like types (not necessarily at integer `T` offsets).
///
/// This is a wrapper around alloc/dealloc to make working with such buffers slightly convenient.
#[derive(Debug)]
pub struct NativeBuffer<T: Sized> {
    ptr: NonNull<u8>,
    layout: Layout,

    // How many bytes of data have been used from the buffer's (layout-defined) capacity.
    len_bytes: usize,

    _phantom: PhantomData<T>,
}

impl<T: Sized> NativeBuffer<T> {
    /// Allocates a buffer of at least the indicated size in bytes, aligned for `T`.
    ///
    /// # Panics
    ///
    /// Panics if `T` is a zero-sized type.
    ///
    /// Panics if the requested size is too large to yield a valid memory layout. Valid
    /// memory layouts must not exceed `isize::MAX` bytes.
    #[must_use]
    pub fn new(min_size_bytes: NonZero<usize>) -> Self {
        // Sometimes Windows will ask for less memory than technically required to fit a T.
        // While this may be valid as long as Rust never tries to read that extra memory (e.g.
        // because the larger union members are not used), it is still feels risky, so
        // we round up to the minimum size required to fit one T to avoid any accidents.
        // In cases where we have multiple T in the buffer, the risk of "reading off the edge"
        // (albeit requiring invalid code) still remains and needs to be guarded against upstack.
        let min_size = size_of::<T>();

        let size_bytes = if min_size_bytes.get() < min_size {
            NonZero::new(min_size).expect("zero-sized T is not supported")
        } else {
            min_size_bytes
        };

        let layout = Layout::from_size_align(size_bytes.get(), align_of::<T>())
            .expect("called with arguments that yielded an invalid memory layout");

        // SAFETY: We must promise to provide a layout of nonzero size.
        // All is well - we asserted above that it is not zero-sized.
        let ptr = unsafe { std::alloc::alloc(layout) };
        let ptr =
            NonNull::new(ptr).expect("allocation failed - the app cannot continue to operate");

        Self {
            ptr,
            layout,
            len_bytes: 0,
            _phantom: PhantomData,
        }
    }

    /// Writes a value at the start of the buffer and declares the buffer length to match.
    pub fn emplace(&mut self, value: T) {
        // SAFETY: Type invariants guarantee the pointer is properly aligned and the borrow checker
        // ensures via `self` that it is valid for writes because we never create references unless
        // a matching `self` reference is held, nor do we hand out pointers via safe APIs.
        unsafe {
            self.ptr.cast::<T>().write(value);
        }

        // SAFETY: We just wrote a T into it, so obviously there is a T worth of data in there.
        unsafe {
            self.set_len_bytes(size_of::<T>());
        }
    }

    /// Allocates a buffer that can exactly fit a `T` and moves the `T` into it.
    ///
    /// # Panics
    ///
    /// Panics if `T` is a zero-sized type.
    #[must_use]
    pub fn from_value(value: T) -> Self {
        let mut buffer = Self::new(
            NonZero::new(size_of::<T>()).expect("cannot create NativeBuffer from zero-sized type"),
        );
        buffer.emplace(value);

        buffer
    }

    /// Allocates a buffer that can fit a number of consecutive instances of `T` and moves the
    /// provided instances into the buffer.
    ///
    /// # Panics
    ///
    /// Panics if `T` is a zero-sized type.
    ///
    /// Panics if the resulting buffer size is too large to yield a valid memory layout. Valid
    /// memory layouts must not exceed `isize::MAX` bytes.
    #[must_use]
    pub fn from_items(items: impl IntoIterator<Item = T>) -> Self {
        // TODO: Use the iterator size hint to avoid this intermediate allocation.
        let items = items.into_iter().collect::<Vec<_>>();

        let mut buffer = Self::new(
            NonZero::new(size_of::<T>().checked_mul(items.len()).expect(
                "if we could put the items in the Vec above, we can put them in the buffer",
            ))
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
            buffer.set_len_bytes(size_of::<T>().checked_mul(count).expect(
                "this is a subset of the capacity so cannot overflow if capacity did not overflow",
            ));
        }

        buffer
    }

    /// Capacity of the buffer, in bytes.
    #[must_use]
    pub fn capacity_bytes(&self) -> usize {
        self.layout.size()
    }

    /// Length of the declared data in the buffer, in bytes.
    #[must_use]
    pub fn len_bytes(&self) -> usize {
        self.len_bytes
    }

    /// Whether the buffer is considered empty.
    ///
    /// This is determined by `set_len()` having been last called with a non-zero value.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len_bytes == 0
    }

    /// Declares the length of the data in the buffer, in bytes.
    ///
    /// # Safety
    ///
    /// The caller guarantees that the indicated number of bytes have been initialized.
    pub unsafe fn set_len_bytes(&mut self, len: usize) {
        assert!(len <= self.capacity_bytes());

        self.len_bytes = len;
    }

    /// Returns a pointer to the start of the buffer.
    #[must_use]
    pub fn as_ptr(&self) -> NonNull<T> {
        self.ptr.cast()
    }

    /// Returns a range of pointers from the start of the buffer to just past the end of the
    /// declared length of the buffer.
    ///
    /// NB! The `end` pointer is not guaranteed to be valid for reads or writes as it may not be
    /// aligned for `T` if the declared length (`len_bytes()`) is not a multiple of `T`'s size.
    /// Its only purpose is to indicate the end of the declared data in the buffer.
    #[must_use]
    pub fn as_data_ptr_range(&self) -> Range<NonNull<T>> {
        let start = self.ptr.cast();

        // SAFETY: The pointer must not go out of bounds of our allocation,
        // which we guarantee because we guard changes to `len` to be in-bounds.
        let end = unsafe { self.ptr.byte_add(self.len_bytes).cast() };

        start..end
    }

    /// Returns a range of pointers from the start of the buffer to the end of the buffer, ignoring
    /// any declared length.
    ///
    /// The `end` pointer is guaranteed to be aligned for `T` but is not valid for reading
    /// or writing because it is at the end of the allocation
    #[must_use]
    pub fn as_capacity_ptr_range(&self) -> Range<NonNull<T>> {
        let start = self.ptr.cast();

        // SAFETY: The pointer must not go out of bounds of our allocation,
        // which we guarantee because we take the size straight from the layout.
        let end = unsafe { self.ptr.byte_add(self.layout.size()).cast() };

        start..end
    }
}

impl<T: Sized> AsMut<MaybeUninit<T>> for NativeBuffer<T> {
    /// Returns an exclusive reference to the first item in the buffer.
    fn as_mut(&mut self) -> &mut MaybeUninit<T> {
        // SAFETY: We are returning MaybeUninit, so it is safe to reference the contents.
        // Borrowing rules are enforced via the borrow on `self`, which is extended here.
        unsafe { self.ptr.cast().as_mut() }
    }
}

impl<T: Sized> AsRef<MaybeUninit<T>> for NativeBuffer<T> {
    /// Returns a shared reference to the first item in the buffer.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_test() {
        let mut buffer = NativeBuffer::from_value(1234_usize);
        assert_eq!(buffer.len_bytes(), size_of::<usize>());
        assert!(!buffer.is_empty());
        assert!(buffer.capacity_bytes() >= size_of::<usize>());

        // SAFETY: We have initialized the first item.
        let value = unsafe { buffer.as_ref().assume_init_read() };
        assert_eq!(value, 1234_usize);

        // SAFETY: We have initialized the first item.
        let value = unsafe { buffer.as_mut().assume_init_read() };
        assert_eq!(value, 1234_usize);

        let ptr = buffer.as_ptr();
        // SAFETY: We have initialized the first item.
        let value = unsafe { ptr.read() };
        assert_eq!(value, 1234_usize);

        let ptr_range = buffer.as_data_ptr_range();
        assert_eq!(ptr_range.start, ptr);

        // Even though the buffer may be any size, this is a pointer pair
        // over the data range and we know the size of the data.
        // SAFETY: We know there must be at least one item of capacity in the buffer,
        // so the expected pointer is not going outside the bounds of the allocation.
        let expected_end = unsafe { ptr.add(1) };
        assert_eq!(ptr_range.end, expected_end);

        // SAFETY: Just setting to zero, always legal.
        unsafe {
            buffer.set_len_bytes(0);
        }

        assert!(buffer.is_empty());

        // There is no data in the buffer anymore, so the data range must be zero-sized.
        let ptr_range = buffer.as_data_ptr_range();
        assert_eq!(ptr_range.start, ptr_range.end);

        // The capacity range is still nonzero, though.
        let capacity_range = buffer.as_capacity_ptr_range();
        assert_eq!(capacity_range.start, ptr);
        assert_ne!(capacity_range.start, capacity_range.end);
    }

    #[test]
    fn from_items() {
        let items: [usize; 4] = [1, 2, 3, 4];
        let buffer = NativeBuffer::from_items(items);

        assert_eq!(buffer.len_bytes(), size_of::<usize>() * items.len());
        assert!(buffer.capacity_bytes() >= size_of::<usize>() * items.len());

        let data_range = buffer.as_data_ptr_range();
        let mut current = data_range.start;

        while current < data_range.end {
            // SAFETY: The loop condition ensures we are in bounds of the data range,
            // so it is valid to read from the buffer at this position.
            let value = unsafe { current.read() };
            assert!(items.contains(&value));

            // SAFETY: The loop condition will prevent any access outside the data range.
            current = unsafe { current.add(1) };
        }
    }
}
