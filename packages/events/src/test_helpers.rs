use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{RawWaker, RawWakerVTable, Waker};

/// A thread-safe waker that tracks whether `wake()` was called.
///
/// Use `testing::ReentrantWakerData` when testing single-threaded
/// (`Local`) event types — its backing `Cell` is `!Sync` and not
/// suitable for thread-safe variants.
pub(crate) struct AtomicWakeTracker {
    woken: AtomicBool,
}

impl AtomicWakeTracker {
    /// The returned `Box` ensures a stable address for the raw waker pointer.
    #[expect(
        clippy::unnecessary_box_returns,
        reason = "the Box ensures a stable address for the raw waker data pointer"
    )]
    pub(crate) fn new() -> Box<Self> {
        Box::new(Self {
            woken: AtomicBool::new(false),
        })
    }

    /// Creates a [`Waker`] backed by this tracker.
    ///
    /// # Safety
    ///
    /// The caller must ensure this `AtomicWakeTracker` outlives all wakers
    /// (and their clones) created from it.
    pub(crate) unsafe fn waker(&self) -> Waker {
        let data: *const () = std::ptr::from_ref(self).cast();

        // SAFETY: The caller ensures this AtomicWakeTracker outlives all
        // wakers created from it.
        unsafe { Waker::from_raw(RawWaker::new(data, &ATOMIC_VTABLE)) }
    }

    pub(crate) fn was_woken(&self) -> bool {
        self.woken.load(Ordering::Relaxed)
    }
}

fn atomic_clone_fn(data: *const ()) -> RawWaker {
    RawWaker::new(data, &ATOMIC_VTABLE)
}

fn atomic_wake_fn(data: *const ()) {
    atomic_wake_by_ref_fn(data);
}

fn atomic_wake_by_ref_fn(data: *const ()) {
    // SAFETY: Validity — `data` was set up by `AtomicWakeTracker::waker()` to point
    // to an `AtomicWakeTracker` that outlives every derived waker (per the safety
    // contract on `waker()`). Aliasing — the type only exposes `&self` methods backed
    // by atomics, so unbounded `&AtomicWakeTracker` references may coexist; no
    // `&mut AtomicWakeTracker` is ever constructed by this type's API.
    let this = unsafe { &*(data as *const AtomicWakeTracker) };
    this.woken.store(true, Ordering::Relaxed);
}

fn atomic_drop_fn(_data: *const ()) {}

static ATOMIC_VTABLE: RawWakerVTable = RawWakerVTable::new(
    atomic_clone_fn,
    atomic_wake_fn,
    atomic_wake_by_ref_fn,
    atomic_drop_fn,
);
