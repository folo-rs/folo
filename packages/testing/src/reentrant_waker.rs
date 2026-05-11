use std::cell::Cell;
use std::fmt;
use std::task::{RawWaker, RawWakerVTable, Waker};

/// A custom waker that executes a caller-supplied closure when woken.
///
/// Used to test that synchronization primitives release their locks
/// before calling `wake()`. If a re-entrant waker accesses the same
/// primitive's internal state while the lock is still held, Miri
/// detects the aliased access as undefined behavior.
///
/// # Thread safety
///
/// This type is `!Send`. The waker and its backing data must stay
/// on a single thread. It can be used with both single-threaded
/// (`Local`) and thread-safe primitive types, as long as all
/// operations happen on one thread.
pub struct ReentrantWakerData {
    action: Box<dyn Fn()>,
    was_woken: Cell<bool>,
}

impl ReentrantWakerData {
    /// Creates a new reentrant waker data with the given action.
    ///
    /// The returned `Box` ensures a stable address for the raw
    /// waker data pointer.
    #[allow(
        clippy::unnecessary_box_returns,
        reason = "the Box ensures a stable address for the raw waker data pointer"
    )]
    pub fn new(action: impl Fn() + 'static) -> Box<Self> {
        Box::new(Self {
            action: Box::new(action),
            was_woken: Cell::new(false),
        })
    }

    /// Creates a [`Waker`] backed by this data.
    ///
    /// # Safety
    ///
    /// The caller must ensure this `ReentrantWakerData` outlives all
    /// wakers (and their clones) created from it. The returned waker
    /// must not be sent to another thread.
    pub unsafe fn waker(&self) -> Waker {
        let data: *const () = std::ptr::from_ref(self).cast();

        // SAFETY: The caller upholds the safety contract.
        unsafe { Waker::from_raw(RawWaker::new(data, &VTABLE)) }
    }

    /// Returns whether `wake()` was called on any waker created
    /// from this data.
    pub fn was_woken(&self) -> bool {
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
    // SAFETY: data points to a valid ReentrantWakerData that outlives
    // this waker per the contract on `ReentrantWakerData::waker()`.
    let this = unsafe { &*(data.cast::<ReentrantWakerData>()) };
    this.was_woken.set(true);
    (this.action)();
}

fn drop_fn(_data: *const ()) {}

static VTABLE: RawWakerVTable = RawWakerVTable::new(clone_fn, wake_fn, wake_by_ref_fn, drop_fn);

impl fmt::Debug for ReentrantWakerData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(std::any::type_name::<Self>())
            .field("was_woken", &self.was_woken.get())
            .finish_non_exhaustive()
    }
}
