//! Allocation wrapper for tracking memory allocations.

use std::alloc::{GlobalAlloc, Layout};
use std::cell::{Cell, OnceCell};
use std::fmt;
#[cfg(feature = "panic_on_next_alloc")]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{self, AtomicU64};
use std::sync::{Arc, LazyLock, Mutex};

use crate::ERR_POISONED_LOCK;

/// Only the per-thread counters updated on each allocation. A global registry of all
/// counters (including those from threads that have since exited) allows summation for
/// process-wide spans without global contention.
#[derive(Debug)]
pub(crate) struct PerThreadCounters {
    bytes: AtomicU64,
    count: AtomicU64,
}

impl PerThreadCounters {
    #[inline]
    const fn new() -> Self {
        Self {
            bytes: AtomicU64::new(0),
            count: AtomicU64::new(0),
        }
    }

    #[inline]
    pub(crate) fn register_allocation(&self, bytes: u64) {
        // Relaxed is sufficient: we only need atomicity, not ordering w.r.t. other memory ops.
        self.bytes.fetch_add(bytes, atomic::Ordering::Relaxed);
        self.count.fetch_add(1, atomic::Ordering::Relaxed);
    }

    #[inline]
    pub(crate) fn bytes(&self) -> u64 {
        self.bytes.load(atomic::Ordering::Relaxed)
    }

    #[inline]
    pub(crate) fn count(&self) -> u64 {
        self.count.load(atomic::Ordering::Relaxed)
    }
}

// Global registry holding Arc references so counters outlive their threads.
// LazyLock gives us one-time initialization without a helper function.
static REGISTRY: LazyLock<Mutex<Vec<Arc<PerThreadCounters>>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

thread_local! {
    // We store a raw pointer to the per-thread counters rather than an Arc directly for two reasons:
    // 1. TLS destructor constraints with the global allocator: If we kept an Arc in TLS, the Arc's Drop
    //    could run during thread teardown while the global allocator is still active, potentially
    //    performing deallocation (and therefore re-entering allocation tracking) at an unsafe point.
    //    Using only a raw pointer avoids any Drop logic during TLS destruction.
    // 2. Avoid recursive tracking during initialization: Setting up the Arc (heap allocation + pushing into
    //    the global registry Vec) itself allocates. If we attempted to track those allocations we would
    //    recurse into the allocator. A small reentrancy guard below disables tracking for that window.
    // Lifetime safety: The Arc is stored in the global REGISTRY which is never cleared, so the pointed-to
    // PerThreadCounters outlive all threads. Hence the raw pointer remains valid for the program lifetime.
    static TLS_COUNTER_PTR: OnceCell<*const PerThreadCounters> = const { OnceCell::new() };
    // Reentrancy guard flag; when true we are in the middle of initializing this thread's counters and
    // must not attempt to record allocations.
    static TLS_INIT_GUARD: Cell<bool> = const { Cell::new(false) };
}

#[inline]
pub(crate) fn get_or_init_thread_counters() -> &'static PerThreadCounters {
    TLS_COUNTER_PTR.with(|cell| {
        if let Some(ptr) = cell.get() {
            // SAFETY: pointer originates from Arc stored in REGISTRY which retains ownership for program lifetime.
            return unsafe { &**ptr };
        }

        TLS_INIT_GUARD.set(true);

        let arc = Arc::new(PerThreadCounters::new());
        let ptr = Arc::as_ptr(&arc);
        // Push Arc to global registry to extend lifetime for program duration.
        REGISTRY.lock().expect(ERR_POISONED_LOCK).push(arc);
        _ = cell.set(ptr);

        TLS_INIT_GUARD.set(false);

        // SAFETY: pointer obtained from Arc::as_ptr for Arc stored in REGISTRY; lifetime extends for program duration.
        unsafe { &*ptr }
    })
}

/// Aggregate totals across all registered threads.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AllocationTotals {
    pub bytes: u64,
    pub count: u64,
}

impl AllocationTotals {
    #[inline]
    pub(crate) const fn zero() -> Self {
        Self { bytes: 0, count: 0 }
    }
}

/// Sum all registered counters (process-wide view at a point in time).
#[inline]
pub(crate) fn allocation_totals() -> AllocationTotals {
    let reg = REGISTRY.lock().expect(ERR_POISONED_LOCK);

    let mut totals = AllocationTotals::zero();
    for c in reg.iter() {
        totals.bytes = totals.bytes.wrapping_add(c.bytes());
        totals.count = totals.count.wrapping_add(c.count());
    }
    totals
}

/// Global flag to control whether the next memory allocation should panic.
/// When set to true, the next allocation attempt will panic and then reset the flag to false.
#[cfg(feature = "panic_on_next_alloc")]
static PANIC_ON_NEXT_ALLOCATION: AtomicBool = AtomicBool::new(false);

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
fn check_and_panic_if_enabled() {}

/// Updates allocation tracking counters for the given size.
/// Only per-thread counters are updated; process-wide views sum them on demand.
fn track_allocation(size: usize) {
    let size_u64: u64 = size.try_into().expect("usize always fits into u64");
    TLS_INIT_GUARD.with(|guard| {
        if guard.get() {
            return; // Skip tracking during initialization path (allocations still occur but are intentionally not recorded).
        }
        let counters = get_or_init_thread_counters();
        counters.register_allocation(size_u64);
    });
}

// Test helper for unit tests where we do not hook the global allocator.
#[cfg(test)]
pub(crate) fn register_fake_allocation(bytes: u64, count: u64) {
    let counters = get_or_init_thread_counters();
    if bytes != 0 {
        counters.bytes.fetch_add(bytes, atomic::Ordering::Relaxed);
    }
    if count != 0 {
        counters.count.fetch_add(count, atomic::Ordering::Relaxed);
    }
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
#[cfg_attr(coverage_nightly, coverage(off))]
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
