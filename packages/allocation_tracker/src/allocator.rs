//! Allocation wrapper for tracking memory usage.

use std::alloc::{GlobalAlloc, Layout, System};
use std::fmt;

use tracking_allocator::Allocator;

/// A wrapper around [`tracking_allocator::Allocator`] that hides the implementation details
/// from users of this crate.
///
/// This allows users to set up allocation tracking without needing to directly depend on
/// or know about the `tracking_allocator` crate.
///
/// # Examples
///
/// ```rust
/// use allocation_tracker::TrackingAllocator;
/// use std::alloc::System;
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
            .field("inner", &"<tracking_allocator::Allocator>")
            .finish()
    }
}

impl TrackingAllocator<System> {
    /// Creates a new tracking allocator that wraps the system allocator.
    ///
    /// This is a convenience method for the common case of wanting to track
    /// allocations while using the system's default allocator.
    #[must_use]
    pub const fn system() -> Self {
        Self {
            inner: Allocator::system(),
        }
    }
}

impl<A: GlobalAlloc> TrackingAllocator<A> {
    /// Creates a new tracking allocator that wraps the provided allocator.
    #[must_use]
    pub const fn new(allocator: A) -> Self {
        Self {
            inner: Allocator::from_allocator(allocator),
        }
    }
}

// SAFETY: We delegate all allocation operations to the inner Allocator,
// which already implements GlobalAlloc safely.
unsafe impl<A: GlobalAlloc> GlobalAlloc for TrackingAllocator<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: We forward the call to the inner allocator which implements GlobalAlloc.
        unsafe { self.inner.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: We forward the call to the inner allocator which implements GlobalAlloc.
        unsafe { self.inner.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: We forward the call to the inner allocator which implements GlobalAlloc.
        unsafe { self.inner.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: We forward the call to the inner allocator which implements GlobalAlloc.
        unsafe { self.inner.realloc(ptr, layout, new_size) }
    }
}
