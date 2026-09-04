//! Allocation wrapper for tracking memory allocations.

use std::alloc::{GlobalAlloc, Layout};
use std::any::type_name;
use std::cell::{Cell, OnceCell};
use std::fmt;
#[cfg(feature = "panic_on_next_alloc")]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{self, AtomicI64, AtomicU64};
use std::sync::{Arc, LazyLock, Mutex};

use crate::ERR_POISONED_LOCK;

/// Only the per-thread counters updated on each allocation. A global registry of all
/// counters (including those from threads that have since exited) allows summation for
/// process-wide spans without global contention.
///
/// Every counter here has exactly one writer — the thread that owns it — so updates are a
/// relaxed load/store pair rather than a `fetch_add`. There is no read-modify-write to make
/// atomic, and avoiding the lock prefix keeps the allocator hot path cheap. The fields stay
/// atomic because [`allocation_totals`] reads them from other threads, which may therefore
/// observe a slightly stale value; the process-wide summation already tolerates that.
#[derive(Debug)]
pub(crate) struct PerThreadCounters {
    bytes: AtomicU64,
    count: AtomicU64,
    outstanding: AtomicI64,
    peak_outstanding: AtomicI64,
}

impl PerThreadCounters {
    #[inline]
    const fn new() -> Self {
        Self {
            bytes: AtomicU64::new(0),
            count: AtomicU64::new(0),
            outstanding: AtomicI64::new(0),
            peak_outstanding: AtomicI64::new(0),
        }
    }

    #[inline]
    pub(crate) fn register_allocation(&self, bytes: u64) {
        self.add_to_totals(bytes);
        self.raise_outstanding(as_delta(bytes));
    }

    #[inline]
    pub(crate) fn register_deallocation(&self, bytes: u64) {
        self.shift_outstanding(as_delta(bytes).wrapping_neg());
    }

    /// Records a reallocation of a block that was `old_bytes` long and is now `new_bytes` long.
    ///
    /// The cumulative total counts the full new size, matching how the allocator reports the
    /// request. Only the difference is outstanding, because the old block is released as part
    /// of the same operation.
    #[inline]
    pub(crate) fn register_reallocation(&self, old_bytes: u64, new_bytes: u64) {
        self.add_to_totals(new_bytes);
        self.raise_outstanding(as_delta(new_bytes).wrapping_sub(as_delta(old_bytes)));
    }

    /// Adds one request of `bytes` to the cumulative totals.
    #[inline]
    fn add_to_totals(&self, bytes: u64) {
        let total = self
            .bytes
            .load(atomic::Ordering::Relaxed)
            .wrapping_add(bytes);
        self.bytes.store(total, atomic::Ordering::Relaxed);

        let count = self.count.load(atomic::Ordering::Relaxed).wrapping_add(1);
        self.count.store(count, atomic::Ordering::Relaxed);
    }

    /// Applies `delta` to the outstanding total and lifts the high-water mark if the result
    /// exceeds it.
    #[inline]
    fn raise_outstanding(&self, delta: i64) {
        let outstanding = self.shift_outstanding(delta);

        if outstanding > self.peak_outstanding.load(atomic::Ordering::Relaxed) {
            self.peak_outstanding
                .store(outstanding, atomic::Ordering::Relaxed);
        }
    }

    /// Applies `delta` to the outstanding total, returning the new value.
    #[inline]
    fn shift_outstanding(&self, delta: i64) -> i64 {
        let outstanding = self
            .outstanding
            .load(atomic::Ordering::Relaxed)
            .wrapping_add(delta);
        self.outstanding
            .store(outstanding, atomic::Ordering::Relaxed);
        outstanding
    }

    #[inline]
    pub(crate) fn bytes(&self) -> u64 {
        self.bytes.load(atomic::Ordering::Relaxed)
    }

    #[inline]
    pub(crate) fn count(&self) -> u64 {
        self.count.load(atomic::Ordering::Relaxed)
    }

    /// Bytes allocated on this thread and not yet freed on this thread.
    ///
    /// May be negative. A thread can free blocks it never allocated: blocks handed to it by
    /// another thread, and blocks allocated before its counters existed or during the
    /// initialization window that deliberately skips tracking. The counter therefore records
    /// this thread's allocator traffic, not the memory it owns.
    #[inline]
    pub(crate) fn outstanding(&self) -> i64 {
        self.outstanding.load(atomic::Ordering::Relaxed)
    }

    /// The high-water mark of [`outstanding`](Self::outstanding).
    ///
    /// Spans own this value: each resets it on entry and restores it on exit, so it means
    /// "the highest level reached since the innermost live span started" rather than an
    /// all-time maximum.
    #[inline]
    pub(crate) fn peak_outstanding(&self) -> i64 {
        self.peak_outstanding.load(atomic::Ordering::Relaxed)
    }

    /// Overwrites the high-water mark, for a span establishing or restoring its baseline.
    #[inline]
    pub(crate) fn set_peak_outstanding(&self, value: i64) {
        self.peak_outstanding
            .store(value, atomic::Ordering::Relaxed);
    }
}

/// Converts an allocation size to the signed representation the outstanding counter uses.
#[inline]
fn as_delta(bytes: u64) -> i64 {
    i64::try_from(bytes)
        .expect("a single allocation cannot exceed isize::MAX bytes, which `Layout` guarantees")
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
            return unsafe { (*ptr).as_ref_unchecked() };
        }

        TLS_INIT_GUARD.set(true);

        let arc = Arc::new(PerThreadCounters::new());
        let ptr = Arc::as_ptr(&arc);
        // Push Arc to global registry to extend lifetime for program duration.
        REGISTRY.lock().expect(ERR_POISONED_LOCK).push(arc);
        _ = cell.set(ptr);

        TLS_INIT_GUARD.set(false);

        // SAFETY: pointer obtained from Arc::as_ptr for Arc stored in REGISTRY; lifetime extends for program duration.
        unsafe { ptr.as_ref_unchecked() }
    })
}

/// This thread's counters, if they already exist.
///
/// The deallocation path uses this instead of [`get_or_init_thread_counters`] because
/// creating the counters allocates and locks, which a free must not do.
#[inline]
fn existing_thread_counters() -> Option<&'static PerThreadCounters> {
    TLS_COUNTER_PTR.with(|cell| {
        // SAFETY: pointer originates from Arc stored in REGISTRY which retains ownership for program lifetime.
        cell.get().map(|ptr| unsafe { (*ptr).as_ref_unchecked() })
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

/// This thread's counters, creating them if this is the thread's first tracked event.
///
/// Returns `None` while the counters are themselves being created, because that work
/// allocates and must not recurse into tracking.
#[inline]
fn thread_counters_for_tracking() -> Option<&'static PerThreadCounters> {
    if let Some(counters) = existing_thread_counters() {
        // Initialization publishes the counter pointer as its last step, so the pointer being
        // present already proves we are not inside the initialization window. Checking the
        // reentrancy guard as well would cost a second thread-local lookup on the hot path.
        return Some(counters);
    }

    if TLS_INIT_GUARD.get() {
        return None;
    }

    Some(get_or_init_thread_counters())
}

/// Updates allocation tracking counters for the given size.
/// Only per-thread counters are updated; process-wide views sum them on demand.
fn track_allocation(size: usize) {
    let size_u64: u64 = size.try_into().expect("usize always fits into u64");

    if let Some(counters) = thread_counters_for_tracking() {
        counters.register_allocation(size_u64);
    }
}

/// Updates tracking counters for a block resized from `old_size` to `new_size`.
fn track_reallocation(old_size: usize, new_size: usize) {
    let old_size_u64: u64 = old_size.try_into().expect("usize always fits into u64");
    let new_size_u64: u64 = new_size.try_into().expect("usize always fits into u64");

    if let Some(counters) = thread_counters_for_tracking() {
        counters.register_reallocation(old_size_u64, new_size_u64);
    }
}

/// Updates tracking counters for a released block of the given size.
///
/// Unlike the allocation paths, this never creates the thread's counters. Doing so allocates
/// an `Arc` and locks the registry, which would re-enter the allocator from inside a free and
/// could consume the one-shot `panic_on_next_alloc` flag. A thread whose first tracked event
/// is a free therefore records nothing, which the signed outstanding counter tolerates.
fn track_deallocation(size: usize) {
    let size_u64: u64 = size.try_into().expect("usize always fits into u64");

    if let Some(counters) = existing_thread_counters() {
        counters.register_deallocation(size_u64);
    }
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
    counters.raise_outstanding(as_delta(bytes));
}

// Test helper for unit tests where we do not hook the global allocator.
#[cfg(test)]
pub(crate) fn register_fake_deallocation(bytes: u64) {
    get_or_init_thread_counters().register_deallocation(bytes);
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

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<A: GlobalAlloc> fmt::Debug for Allocator<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
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
    // Only ever executed in const context, which is not covered by coverage measurement.
    #[cfg_attr(coverage_nightly, coverage(off))]
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
    // Only ever executed in const context, which is not covered by coverage measurement.
    #[cfg_attr(coverage_nightly, coverage(off))]
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

        // SAFETY: We forward the call to the underlying allocator which implements GlobalAlloc.
        let ptr = unsafe { self.inner.alloc(layout) };

        // A failed allocation reserves nothing, so recording it would permanently inflate the
        // outstanding total against a block that will never be freed.
        if !ptr.is_null() {
            track_allocation(layout.size());
        }

        ptr
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: We forward the call to the underlying allocator which implements GlobalAlloc.
        unsafe { self.inner.dealloc(ptr, layout); }

        track_deallocation(layout.size());
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        check_and_panic_if_enabled();

        // SAFETY: We forward the call to the underlying allocator which implements GlobalAlloc.
        let ptr = unsafe { self.inner.alloc_zeroed(layout) };

        if !ptr.is_null() {
            track_allocation(layout.size());
        }

        ptr
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        check_and_panic_if_enabled();

        // SAFETY: We forward the call to the underlying allocator which implements GlobalAlloc.
        let new_ptr = unsafe { self.inner.realloc(ptr, layout, new_size) };

        // On failure the original block is still live and unchanged, so no counter moves.
        if !new_ptr.is_null() {
            track_reallocation(layout.size(), new_size);
        }

        new_ptr
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::alloc::System;
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::{iter, ptr, thread};

    use super::*;

    // Static assertions for thread safety.
    static_assertions::assert_impl_all!(Allocator<System>: Send, Sync);
    static_assertions::assert_impl_all!(PerThreadCounters: Send, Sync);

    // Static assertions for unwind safety.
    static_assertions::assert_impl_all!(
        Allocator<System>: UnwindSafe, RefUnwindSafe
    );

    /// A `GlobalAlloc` that fails every request, for exercising the allocation-failure paths.
    ///
    /// It never hands out memory and never inspects the pointers it is given, so passing it a
    /// block owned by another allocator cannot cause harm.
    struct FailingAllocator;

    // SAFETY: Returning null is the documented way to report allocation failure, and we never
    // claim ownership of or dereference any pointer.
    unsafe impl GlobalAlloc for FailingAllocator {
        unsafe fn alloc(&self, _layout: Layout) -> *mut u8 {
            ptr::null_mut()
        }

        unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {}

        unsafe fn realloc(&self, _ptr: *mut u8, _layout: Layout, _new_size: usize) -> *mut u8 {
            ptr::null_mut()
        }
    }

    /// An arbitrary layout, large enough that a real allocation is unlikely to be optimized away.
    fn test_layout(size: usize) -> Layout {
        Layout::from_size_align(size, 8).unwrap()
    }

    #[test]
    fn outstanding_follows_allocations_and_deallocations() {
        let counters = PerThreadCounters::new();

        assert_eq!(counters.outstanding(), 0);

        counters.register_allocation(100);
        counters.register_allocation(50);
        assert_eq!(counters.outstanding(), 150);

        counters.register_deallocation(100);
        assert_eq!(counters.outstanding(), 50);

        // Cumulative totals are unaffected by frees.
        assert_eq!(counters.bytes(), 150);
        assert_eq!(counters.count(), 2);
    }

    #[test]
    fn outstanding_goes_negative_when_freeing_untracked_memory() {
        let counters = PerThreadCounters::new();

        // A thread can free a block that another thread allocated, or one allocated before
        // its counters existed.
        counters.register_deallocation(100);

        assert_eq!(counters.outstanding(), -100);
        assert_eq!(counters.peak_outstanding(), 0);
    }

    #[test]
    fn peak_holds_high_water_mark() {
        let counters = PerThreadCounters::new();

        counters.register_allocation(100);
        counters.register_allocation(50);
        assert_eq!(counters.peak_outstanding(), 150);

        counters.register_deallocation(150);
        counters.register_allocation(20);

        assert_eq!(counters.outstanding(), 20);
        assert_eq!(counters.peak_outstanding(), 150);
    }

    #[test]
    fn reallocation_adjusts_outstanding_by_size_difference() {
        let counters = PerThreadCounters::new();

        counters.register_allocation(100);
        counters.register_reallocation(100, 300);

        assert_eq!(counters.outstanding(), 300);
        assert_eq!(counters.peak_outstanding(), 300);

        counters.register_reallocation(300, 80);

        assert_eq!(counters.outstanding(), 80);
        assert_eq!(counters.peak_outstanding(), 300);

        // The cumulative total counts each request at its full requested size.
        assert_eq!(counters.bytes(), 480);
        assert_eq!(counters.count(), 3);
    }

    #[test]
    fn set_peak_outstanding_overwrites_watermark() {
        let counters = PerThreadCounters::new();

        counters.register_allocation(100);
        counters.set_peak_outstanding(40);

        assert_eq!(counters.peak_outstanding(), 40);
    }

    #[test]
    fn allocation_and_deallocation_move_outstanding() {
        const SIZE: usize = 1024;

        let allocator = Allocator::new(System);
        let layout = test_layout(SIZE);
        let counters = get_or_init_thread_counters();

        let before = counters.outstanding();

        // SAFETY: The layout has a non-zero size and a power-of-two alignment.
        let ptr = unsafe { allocator.alloc(layout) };
        let after_alloc = counters.outstanding();

        // SAFETY: The block was just obtained from this allocator with this exact layout.
        unsafe { allocator.dealloc(ptr, layout); }
        let after_dealloc = counters.outstanding();

        assert!(!ptr.is_null());
        assert_eq!(after_alloc.wrapping_sub(before), 1024);
        assert_eq!(after_dealloc, before);
    }

    #[test]
    fn failed_allocation_does_not_move_counters() {
        let allocator = Allocator::new(FailingAllocator);
        let layout = test_layout(1024);
        let counters = get_or_init_thread_counters();

        let before_bytes = counters.bytes();
        let before_outstanding = counters.outstanding();

        // SAFETY: The layout has a non-zero size and a power-of-two alignment.
        let ptr = unsafe { allocator.alloc(layout) };

        let after_bytes = counters.bytes();
        let after_outstanding = counters.outstanding();

        assert!(ptr.is_null());
        assert_eq!(before_bytes, after_bytes);
        assert_eq!(before_outstanding, after_outstanding);
    }

    #[test]
    fn failed_reallocation_does_not_move_counters() {
        let layout = test_layout(64);
        let system = System;

        // SAFETY: The layout has a non-zero size and a power-of-two alignment.
        let block = unsafe { system.alloc(layout) };
        assert!(!block.is_null());

        let allocator = Allocator::new(FailingAllocator);
        let counters = get_or_init_thread_counters();

        let before_bytes = counters.bytes();
        let before_outstanding = counters.outstanding();

        // SAFETY: `FailingAllocator` never inspects the pointer, so handing it a block owned
        // by the system allocator leaves that block untouched and still owned by us.
        let grown = unsafe { allocator.realloc(block, layout, 256) };

        let after_bytes = counters.bytes();
        let after_outstanding = counters.outstanding();

        // SAFETY: The block came from the system allocator with this exact layout and the
        // failed reallocation did not release it.
        unsafe { system.dealloc(block, layout); }

        assert!(grown.is_null());
        assert_eq!(before_bytes, after_bytes);
        assert_eq!(before_outstanding, after_outstanding);
    }

    #[test]
    fn deallocation_on_untracked_thread_creates_no_counters() {
        // The unit test binary does not install the tracking allocator globally, so a fresh
        // thread has no counters until this test creates activity on purpose.
        thread::spawn(|| {
            assert!(existing_thread_counters().is_none());

            let layout = test_layout(64);
            let system = System;

            // SAFETY: The layout has a non-zero size and a power-of-two alignment.
            let block = unsafe { system.alloc(layout) };
            assert!(!block.is_null());

            let allocator = Allocator::new(System);

            // SAFETY: The block came from the system allocator with this exact layout, and
            // the tracking allocator forwards the release to that same allocator.
            unsafe { allocator.dealloc(block, layout); }

            // Creating counters allocates and locks the registry, which a free must never do.
            assert!(existing_thread_counters().is_none());
        })
        .join()
        .unwrap();
    }

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

    // Multithreaded tests exercising concurrent access to PerThreadCounters and the
    // global REGISTRY. These are designed to run under Miri to detect data races in the
    // atomic operations and TLS initialization paths.

    #[test]
    fn concurrent_threads_register_and_totals_reflect_all() {
        const THREADS: usize = 4;
        const BYTES_PER_THREAD: u64 = 100;
        const COUNT_PER_THREAD: u64 = 10;

        // Record the baseline to account for allocations from other tests,
        // since the global REGISTRY is shared across all tests.
        let baseline = allocation_totals();

        let handles: Vec<_> = iter::repeat_with(|| {
            thread::spawn(move || {
                register_fake_allocation(BYTES_PER_THREAD, COUNT_PER_THREAD);
            })
        })
        .take(THREADS)
        .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        let final_totals = allocation_totals();

        // The delta must be at least what we added. It may be higher due to
        // real allocations from the test infrastructure (thread spawning, etc.).
        let bytes_delta = final_totals.bytes.wrapping_sub(baseline.bytes);
        let count_delta = final_totals.count.wrapping_sub(baseline.count);

        assert!(bytes_delta >= THREADS as u64 * BYTES_PER_THREAD);
        assert!(count_delta >= THREADS as u64 * COUNT_PER_THREAD);
    }

    #[test]
    fn concurrent_register_and_read_totals() {
        const WRITER_THREADS: usize = 4;
        const ALLOCS_PER_WRITER: u64 = 10;
        const BYTES_PER_ALLOC: u64 = 50;

        // One set of threads registers allocations while another reads
        // totals concurrently. This exercises concurrent atomic reads of
        // PerThreadCounters while other threads perform atomic writes.
        let baseline = allocation_totals();

        // Spawn the reader thread first so it is already running when
        // the writers start, maximizing concurrent read/write overlap.
        let reader = thread::spawn(move || {
            for _ in 0..20 {
                let _totals = allocation_totals();
            }
        });

        let writers: Vec<_> = iter::repeat_with(|| {
            thread::spawn(move || {
                for _ in 0..ALLOCS_PER_WRITER {
                    register_fake_allocation(BYTES_PER_ALLOC, 1);
                }
            })
        })
        .take(WRITER_THREADS)
        .collect();

        for handle in writers {
            handle.join().unwrap();
        }
        reader.join().unwrap();

        let final_totals = allocation_totals();
        let bytes_delta = final_totals.bytes.wrapping_sub(baseline.bytes);
        assert!(bytes_delta >= WRITER_THREADS as u64 * ALLOCS_PER_WRITER * BYTES_PER_ALLOC);
    }
}
