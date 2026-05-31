//! Live observation data for one registered future, plus the raw waker
//! plumbing that ties it to the production primitive.
//!
//! Each registered future has its own [`WakerTracker`] behind an
//! [`Rc`](std::rc::Rc), pointed to by a custom [`Waker`] built via the
//! raw-waker API. The waker is invoked by the production code during
//! `event.set()` drains; that invocation:
//!
//! * Bumps [`WakerTracker::wake_count`] (the *observation* of how many
//!   times the waker actually fired).
//! * Runs the installed action, which may reentrantly call back into the
//!   harness to exercise reentrancy paths.
//!
//! The matching *expected* upper bound on `wake_count` lives in
//! [`SlotModel::expected_max_wakes`](super::model::SlotModel) — kept on
//! the model side because budgets are assigned by the harness, not by
//! the production code.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::task::{RawWaker, RawWakerVTable, Waker};
use std::thread::{self, ThreadId};

/// Per-slot waker-side data shared between the harness and the
/// production code via a raw waker pointer.
pub(super) struct WakerTracker {
    /// Number of times this slot's waker has actually been invoked.
    /// Mutated by [`wake_by_ref_fn`] running inside `event.set()`.
    pub(super) wake_count: Cell<u32>,
    /// Reentrant action invoked synchronously inside `wake()`. Wrapped
    /// in `RefCell` because installing the action requires a
    /// back-reference to the harness that does not exist at tracker
    /// creation time.
    pub(super) action: RefCell<Box<dyn Fn()>>,
    /// Thread that created this tracker. Every vtable function asserts
    /// the current thread matches before touching the `!Sync` interior.
    /// This converts what would otherwise be UB (an `Rc`-backed waker
    /// observed from another thread) into a deterministic panic during
    /// testing. The harness itself is single-threaded by construction;
    /// the assertion guards against future modifications that might
    /// spawn tasks.
    creator_thread: ThreadId,
}

impl WakerTracker {
    pub(super) fn new() -> Rc<Self> {
        Rc::new(Self {
            wake_count: Cell::new(0),
            action: RefCell::new(Box::new(|| {})),
            creator_thread: thread::current().id(),
        })
    }

    pub(super) fn install_action(self: &Rc<Self>, action: Box<dyn Fn()>) {
        *self.action.borrow_mut() = action;
    }
}

/// Builds a [`Waker`] that points at `tracker`.
///
/// The raw pointer borrows from a clone of the `Rc`, kept alive by
/// leaking the clone for the duration of every outstanding `Waker`.
/// The leak is recovered when the waker is dropped via [`drop_fn`].
///
/// `WakerTracker` is `!Send`/`!Sync` because it holds `Cell` and
/// `RefCell`. The `Waker` type is `Send + Sync` by trait, so this
/// constructor relies on a runtime guardrail: every vtable function
/// asserts that the current thread matches `tracker.creator_thread`.
/// Single-threaded misuse panics deterministically; any future change
/// that accidentally moves a waker off-thread will be caught instead of
/// becoming UB.
pub(super) fn make_waker(tracker: &Rc<WakerTracker>) -> Waker {
    let cloned = Rc::clone(tracker);
    let raw = Rc::into_raw(cloned);
    let data: *const () = raw.cast::<()>();

    // SAFETY: Validity — `data` was just produced from `Rc::into_raw`
    // and owns a strong count; the vtable maintains that count
    // (`clone_fn` increments, `drop_fn` decrements), so the pointee
    // stays alive for the duration of every outstanding `Waker`.
    // Aliasing — vtable functions only ever construct shared
    // `&WakerTracker` references, never `&mut`, so reentrant wakes may
    // freely create additional shared references without violating
    // Rust's aliasing rules. The `!Sync` interior (`Cell` and
    // `RefCell`) is protected by `assert_creator_thread`, which pins
    // every vtable invocation to the thread that created the tracker —
    // preventing cross-thread observation of the single-threaded
    // interior mutability primitives.
    unsafe { Waker::from_raw(RawWaker::new(data, &VTABLE)) }
}

fn clone_fn(data: *const ()) -> RawWaker {
    assert_creator_thread(data);
    // SAFETY: `data` was produced by `Rc::into_raw` in `make_waker` or
    // by a prior `clone_fn` invocation; either way the pointer owns a
    // strong count. `Rc::increment_strong_count` requires such a
    // pointer.
    unsafe {
        Rc::<WakerTracker>::increment_strong_count(data.cast::<WakerTracker>());
    }
    RawWaker::new(data, &VTABLE)
}

fn wake_fn(data: *const ()) {
    wake_by_ref_fn(data);
    drop_fn(data);
}

fn wake_by_ref_fn(data: *const ()) {
    assert_creator_thread(data);
    // SAFETY: Validity — `data` owns a strong count on a `WakerTracker`
    // (see `clone_fn` / `make_waker`), so the pointee is alive for the
    // duration of this call. Aliasing — we only construct a shared
    // `&WakerTracker`; reentrant invocations may construct further
    // shared references, which is sound. The `!Sync` interior (`Cell`
    // and `RefCell`) is safe to touch here because
    // `assert_creator_thread` confines this call to the tracker's
    // creator thread, so no cross-thread observation occurs.
    let tracker = unsafe { &*data.cast::<WakerTracker>() };
    tracker
        .wake_count
        .set(tracker.wake_count.get().saturating_add(1));
    // The action may re-enter the harness. The harness's contract is
    // that no harness borrow is held while event methods are invoked,
    // so the action can freely borrow.
    let action = tracker.action.borrow();
    (action)();
}

fn drop_fn(data: *const ()) {
    assert_creator_thread(data);
    // SAFETY: `data` was produced by `Rc::into_raw` in `make_waker` or
    // by a `clone_fn` increment; reconstructing the `Rc` here
    // decrements the strong count and (eventually) frees the
    // `WakerTracker` when no references remain.
    unsafe {
        drop(Rc::<WakerTracker>::from_raw(data.cast::<WakerTracker>()));
    }
}

/// Asserts that the calling thread is the one that created the tracker
/// behind `data`. Panics otherwise. Reading the `creator_thread` field
/// is safe from any thread because `ThreadId` is `Copy + Sync`, and the
/// field is never mutated after construction.
fn assert_creator_thread(data: *const ()) {
    // SAFETY: Validity — `data` owns a strong count on a `WakerTracker`
    // (see `make_waker` / `clone_fn`), so the pointee is alive.
    // Aliasing — we only construct a shared `&WakerTracker` and only
    // read the `Sync` `ThreadId` field (never the `!Sync`
    // `Cell`/`RefCell` fields), so this is sound even from a thread
    // other than the creator and even when other shared references to
    // the same tracker coexist.
    let tracker = unsafe { &*data.cast::<WakerTracker>() };
    assert_eq!(
        tracker.creator_thread,
        thread::current().id(),
        "proptest waker accessed from a thread other than its creator",
    );
}

static VTABLE: RawWakerVTable = RawWakerVTable::new(clone_fn, wake_fn, wake_by_ref_fn, drop_fn);
