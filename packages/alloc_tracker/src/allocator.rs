//! Allocation wrapper for tracking memory allocations.

use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::fmt;
use std::sync::atomic::{self, AtomicU64};

// The tracking allocator works with static data all over the place, so this is how it be.
// This is global state - we could theoretically optimize via thread-local counter but that
// might only matter in extremely allocation-heavy scenarios which are not a priority (yet?).
pub(crate) static TOTAL_BYTES_ALLOCATED: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// Thread-local counter for tracking allocations within the current thread.
    /// This allows for thread-specific allocation tracking when using measure_thread().
    pub(crate) static THREAD_BYTES_ALLOCATED: Cell<u64> = const { Cell::new(0) };
}

/// A memory allocator that enables tracking of memory allocations and deallocations.
///
/// This allocator wraps any [`GlobalAlloc`] implementation to provide allocation tracking
/// capabilities while maintaining the same allocation behavior and performance characteristics
/// as the underlying allocator.
///
/// # Examples
///
/// ```rust
/// use alloc_tracker::Allocator;
///
/// #[global_allocator]
/// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
/// ```
pub struct Allocator<A: GlobalAlloc> {
    inner: A,
}

impl<A: GlobalAlloc> fmt::Debug for Allocator<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Allocator")
            .field("inner", &"<allocator>")
            .finish()
    }
}

impl Allocator<std::alloc::System> {
    /// Creates a new tracking allocator using the system's default allocator.
    ///
    /// This is a convenience method for the common case of wanting to track
    /// allocations without changing the underlying allocation strategy.
    #[must_use]
    pub const fn system() -> Self {
        Self {
            inner: std::alloc::System,
        }
    }
}

impl<A: GlobalAlloc> Allocator<A> {
    /// Creates a new tracking allocator that enables allocation tracking for the provided allocator.
    ///
    /// The resulting allocator will have the same performance and behavior characteristics
    /// as the underlying allocator, with the addition of allocation tracking capabilities.
    #[must_use]
    pub const fn new(allocator: A) -> Self {
        Self { inner: allocator }
    }
}

// SAFETY: We delegate all allocation operations to the underlying allocator,
// which already implements GlobalAlloc safely, while adding tracking functionality.
unsafe impl<A: GlobalAlloc> GlobalAlloc for Allocator<A> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size_u64 = layout
            .size()
            .try_into()
            .expect("usize always fits into u64");

        TOTAL_BYTES_ALLOCATED.fetch_add(size_u64, atomic::Ordering::Relaxed);

        THREAD_BYTES_ALLOCATED.with(|counter| {
            counter.set(counter.get().wrapping_add(size_u64));
        });

        // SAFETY: We forward the call to the underlying allocator which implements GlobalAlloc.
        unsafe { self.inner.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: We forward the call to the underlying allocator which implements GlobalAlloc.
        unsafe { self.inner.dealloc(ptr, layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let size_u64 = layout
            .size()
            .try_into()
            .expect("usize always fits into u64");

        TOTAL_BYTES_ALLOCATED.fetch_add(size_u64, atomic::Ordering::Relaxed);

        THREAD_BYTES_ALLOCATED.with(|counter| {
            counter.set(counter.get().wrapping_add(size_u64));
        });

        // SAFETY: We forward the call to the underlying allocator which implements GlobalAlloc.
        unsafe { self.inner.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let new_size_u64 = new_size.try_into().expect("usize always fits into u64");

        TOTAL_BYTES_ALLOCATED.fetch_add(new_size_u64, atomic::Ordering::Relaxed);

        THREAD_BYTES_ALLOCATED.with(|counter| {
            counter.set(counter.get().wrapping_add(new_size_u64));
        });

        // SAFETY: We forward the call to the underlying allocator which implements GlobalAlloc.
        unsafe { self.inner.realloc(ptr, layout, new_size) }
    }
}
