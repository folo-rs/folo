//! Allocation wrapper for tracking memory usage.

use std::alloc::{GlobalAlloc, Layout, System};
use std::fmt;

use tracking_allocator::Allocator;

/// A memory allocator that enables tracking of memory allocations and deallocations.
///
/// This allocator wraps any [`GlobalAlloc`] implementation to provide allocation tracking
/// capabilities while maintaining the same allocation behavior and performance characteristics
/// as the underlying allocator.
///
/// # Examples
///
/// ```rust
/// use std::alloc::System;
///
/// use allocation_tracker::TrackingAllocator;
///
/// #[global_allocator]
/// static ALLOCATOR: TrackingAllocator<System> = TrackingAllocator::system();
/// ```
pub struct TrackingAllocator<A: GlobalAlloc> {
    inner: Allocator<A>,
}

impl<A: GlobalAlloc> fmt::Debug for TrackingAllocator<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TrackingAllocator")
            .field("inner", &"<allocator>")
            .finish()
    }
}

impl TrackingAllocator<System> {
    /// Creates a new tracking allocator using the system's default allocator.
    ///
    /// This is a convenience method for the common case of wanting to track
    /// allocations without changing the underlying allocation strategy.
    #[must_use]
    pub const fn system() -> Self {
        Self {
            inner: Allocator::system(),
        }
    }
}

impl<A: GlobalAlloc> TrackingAllocator<A> {
    /// Creates a new tracking allocator that enables allocation tracking for the provided allocator.
    ///
    /// The resulting allocator will have the same performance and behavior characteristics
    /// as the underlying allocator, with the addition of allocation tracking capabilities.
    #[must_use]
    pub const fn new(allocator: A) -> Self {
        Self {
            inner: Allocator::from_allocator(allocator),
        }
    }
}

// SAFETY: We delegate all allocation operations to the underlying allocator,
// which already implements GlobalAlloc safely, while adding tracking functionality.
unsafe impl<A: GlobalAlloc> GlobalAlloc for TrackingAllocator<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: We forward the call to the underlying allocator which implements GlobalAlloc.
        unsafe { self.inner.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: We forward the call to the underlying allocator which implements GlobalAlloc.
        unsafe { self.inner.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: We forward the call to the underlying allocator which implements GlobalAlloc.
        unsafe { self.inner.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: We forward the call to the underlying allocator which implements GlobalAlloc.
        unsafe { self.inner.realloc(ptr, layout, new_size) }
    }
}
