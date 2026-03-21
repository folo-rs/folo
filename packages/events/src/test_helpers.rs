use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{RawWaker, RawWakerVTable, Waker};

/// A custom waker that executes a caller-supplied closure when woken.
///
/// This is used to test that `wake()` is never called while holding borrows
/// on interior-mutable state (e.g. `UnsafeCell<WaiterList>`). If a re-entrant
/// waker accesses the same state, Miri detects the aliased access as UB.
///
/// # Thread safety
///
/// This type is `!Send` and must only be used with single-threaded (`Local`)
/// event types. The backing state uses `Cell` which is not thread-safe.
/// For thread-safe event types, use [`AtomicWakeTracker`] instead.
pub(crate) struct ReentrantWakerData {
    action: Box<dyn Fn()>,
    was_woken: Cell<bool>,
}

impl ReentrantWakerData {
    /// The returned `Box` ensures a stable address for the waker data pointer.
    #[expect(
        clippy::unnecessary_box_returns,
        reason = "the Box ensures a stable address for the raw waker data pointer"
    )]
    pub(crate) fn new(action: impl Fn() + 'static) -> Box<Self> {
        Box::new(Self {
            action: Box::new(action),
            was_woken: Cell::new(false),
        })
    }

    /// Creates a [`Waker`] backed by this data.
    ///
    /// # Safety
    ///
    /// * The caller must ensure this `ReentrantWakerData` outlives all wakers
    ///   (and their clones) created from it.
    /// * The returned [`Waker`] (and any clones) must not be sent to or used
    ///   from any thread other than the one that created it. The backing
    ///   state uses [`Cell`] and a non-thread-safe closure.
    pub(crate) unsafe fn waker(&self) -> Waker {
        let data: *const () = std::ptr::from_ref(self).cast();

        // SAFETY: The caller upholds the safety contract of `waker()`:
        // the data outlives all wakers, and the waker is only used on
        // the creating thread.
        unsafe { Waker::from_raw(RawWaker::new(data, &VTABLE)) }
    }

    pub(crate) fn was_woken(&self) -> bool {
        self.was_woken.get()
    }
}

fn clone_fn(data: *const ()) -> RawWaker {
    RawWaker::new(data, &VTABLE)
}

fn wake_fn(data: *const ()) {
    wake_by_ref_fn(data);
}

fn wake_by_ref_fn(data: *const ()) {
    // SAFETY: data points to a valid ReentrantWakerData that outlives this
    // waker per the contract on `ReentrantWakerData::waker()`.
    let this = unsafe { &*(data as *const ReentrantWakerData) };
    this.was_woken.set(true);
    (this.action)();
}

fn drop_fn(_data: *const ()) {}

static VTABLE: RawWakerVTable = RawWakerVTable::new(clone_fn, wake_fn, wake_by_ref_fn, drop_fn);

/// A thread-safe waker that tracks whether `wake()` was called.
///
/// Unlike [`ReentrantWakerData`], this type is `Send + Sync` and uses atomic
/// operations, making it suitable for testing thread-safe event types.
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
    /// The caller must ensure this tracker outlives all wakers created from it.
    pub(crate) fn waker(&self) -> Waker {
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
    // SAFETY: data points to a valid AtomicWakeTracker that outlives this
    // waker per the contract on `AtomicWakeTracker::waker()`.
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
