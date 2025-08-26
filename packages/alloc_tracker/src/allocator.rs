//! Allocation wrapper for tracking memory allocations.

use std::alloc::{GlobalAlloc, Layout};
use std::cell::Cell;
use std::fmt;
#[cfg(feature = "panic_on_next_alloc")]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{self, AtomicU64};

// The tracking allocator works with static data all over the place, so this is how it be.
// This is global state - we could theoretically optimize via thread-local counter but that
// might only matter in extremely allocation-heavy scenarios which are not a priority (yet?).
pub(crate) static TOTAL_BYTES_ALLOCATED: AtomicU64 = AtomicU64::new(0);

/// Global counter for tracking the total number of allocations across all threads.
pub(crate) static TOTAL_ALLOCATIONS_COUNT: AtomicU64 = AtomicU64::new(0);

/// Global flag to control whether the next memory allocation should panic.
/// When set to true, the next allocation attempt will panic and then reset the flag to false.
#[cfg(feature = "panic_on_next_alloc")]
static PANIC_ON_NEXT_ALLOCATION: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// Thread-local counter for tracking allocations within the current thread.
    /// This allows for thread-specific allocation tracking when using measure_thread().
    pub(crate) static THREAD_BYTES_ALLOCATED: Cell<u64> = const { Cell::new(0) };

    /// Thread-local counter for tracking the number of allocations within the current thread.
    /// This allows for thread-specific allocation count tracking when using measure_thread().
    pub(crate) static THREAD_ALLOCATIONS_COUNT: Cell<u64> = const { Cell::new(0) };
}

/// Controls whether the next memory allocation should panic.
///
/// When enabled, the next attempt to allocate memory will panic with a descriptive message
/// and then automatically reset the flag to false. This "one-shot" behavior is useful for
/// tracking down unexpected allocations in performance-critical code sections.
///
/// This function is only available when the `panic_on_next_alloc` feature is enabled.
///
/// # Arguments
///
/// * `enabled` - Whether to enable panic-on-next-allocation behavior
///
/// # Examples
///
/// ```rust
/// use alloc_tracker::{Allocator, panic_on_next_alloc};
///
/// #[global_allocator]
/// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
///
/// fn main() {
///     // Enable panic on next allocation
///     panic_on_next_alloc(true);
///
///     // This would panic (and reset the flag):
///     // let _vec = vec![1, 2, 3];
///
///     // Subsequent allocations are now safe again:
///     // let _another_vec = vec![4, 5, 6]; // This would work
/// }
/// ```
#[cfg(feature = "panic_on_next_alloc")]
pub fn panic_on_next_alloc(enabled: bool) {
    PANIC_ON_NEXT_ALLOCATION.store(enabled, atomic::Ordering::Relaxed);
}

/// Checks if panic-on-next-allocation is enabled and panics if so, automatically resetting the flag.
/// This is called before any allocation operation to implement the one-shot panic behavior.
#[cfg(feature = "panic_on_next_alloc")]
fn check_and_panic_if_enabled() {
    // Check if we should panic on this allocation and reset flag if so
    #[expect(
        clippy::manual_assert,
        reason = "We need to atomically swap the flag, not just check it"
    )]
    if PANIC_ON_NEXT_ALLOCATION.swap(false, atomic::Ordering::Relaxed) {
        panic!("Memory allocation attempted while panic-on-next-allocation was enabled");
    }
}

/// No-op version when `panic_on_next_alloc` feature is disabled.
#[cfg(not(feature = "panic_on_next_alloc"))]
#[inline]
fn check_and_panic_if_enabled() {
}

/// Updates allocation tracking counters for the given size.
/// This tracks both global and thread-local allocation statistics for both bytes and count.
fn track_allocation(size: usize) {
    let size_u64 = size.try_into().expect("usize always fits into u64");

    TOTAL_BYTES_ALLOCATED.fetch_add(size_u64, atomic::Ordering::Relaxed);
    TOTAL_ALLOCATIONS_COUNT.fetch_add(1, atomic::Ordering::Relaxed);

    THREAD_BYTES_ALLOCATED.with(|counter| {
        counter.set(counter.get().wrapping_add(size_u64));
    });

    THREAD_ALLOCATIONS_COUNT.with(|counter| {
        counter.set(counter.get().wrapping_add(1));
    });
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
    #[inline]
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
    #[inline]
    pub const fn new(allocator: A) -> Self {
        Self { inner: allocator }
    }
}

// SAFETY: We delegate all allocation operations to the underlying allocator,
// which already implements GlobalAlloc safely, while adding tracking functionality.
unsafe impl<A: GlobalAlloc> GlobalAlloc for Allocator<A> {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        check_and_panic_if_enabled();
        track_allocation(layout.size());

        // SAFETY: We forward the call to the underlying allocator which implements GlobalAlloc.
        unsafe { self.inner.alloc(layout) }
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: We forward the call to the underlying allocator which implements GlobalAlloc.
        unsafe { self.inner.dealloc(ptr, layout) }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        check_and_panic_if_enabled();
        track_allocation(layout.size());

        // SAFETY: We forward the call to the underlying allocator which implements GlobalAlloc.
        unsafe { self.inner.alloc_zeroed(layout) }
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        check_and_panic_if_enabled();
        track_allocation(new_size);

        // SAFETY: We forward the call to the underlying allocator which implements GlobalAlloc.
        unsafe { self.inner.realloc(ptr, layout, new_size) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Static assertions for thread safety
    static_assertions::assert_impl_all!(Allocator<std::alloc::System>: Send, Sync);

    #[test]
    #[cfg(feature = "panic_on_next_alloc")]
    fn panic_on_next_alloc_can_be_enabled_and_disabled() {
        // Default state should be disabled
        assert!(!PANIC_ON_NEXT_ALLOCATION.load(atomic::Ordering::Relaxed));

        // Enable panic on next allocation
        panic_on_next_alloc(true);
        assert!(PANIC_ON_NEXT_ALLOCATION.load(atomic::Ordering::Relaxed));

        // Disable panic on next allocation
        panic_on_next_alloc(false);
        assert!(!PANIC_ON_NEXT_ALLOCATION.load(atomic::Ordering::Relaxed));
    }
}
