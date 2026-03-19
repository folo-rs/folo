use std::cell::Cell;
use std::task::{RawWaker, RawWakerVTable, Waker};

/// A custom waker that executes a caller-supplied closure when woken.
///
/// This is used to test that `wake()` is never called while holding borrows
/// on interior-mutable state (e.g. `UnsafeCell<WaiterList>`). If a re-entrant
/// waker accesses the same state, Miri detects the aliased access as UB.
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
    /// The caller must ensure this `ReentrantWakerData` outlives all wakers
    /// (and their clones) created from it.
    pub(crate) fn waker(&self) -> Waker {
        let data: *const () = std::ptr::from_ref(self).cast();

        // SAFETY: The caller ensures this ReentrantWakerData outlives all
        // wakers created from it. Our vtable functions correctly handle the
        // data pointer.
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
