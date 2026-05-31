//! Shared property-based test harness for manual-reset events.
//!
//! This module captures the operation grammar, harness state, and
//! invariants used by the proptest pilots in
//! [`local_manual_proptest`](crate::local_manual_proptest) and
//! [`manual_proptest`](crate::manual_proptest). Both pilots exercise
//! single-threaded reentrancy properties of their respective manual-
//! reset event primitives; multithreaded race exploration is out of
//! scope here and lives in the dedicated multithreaded tests in
//! `manual` and `auto`.
//!
//! See GitHub issue #149 for the original pilot motivation.
//!
//! The four observable properties (drawn from the events' contracts):
//!
//! 1. **No lost notifications.** Every future that is `Pending` at
//!    the instant an effective `set()` call begins must observe
//!    `Ready` on its next poll, even if a reentrant `reset()` runs
//!    inside the drain loop.
//! 2. **Wake budget.** Each future's waker is invoked at most once
//!    per effective `set()` while the future is in the `Pending`
//!    state. After the future is consumed (`Ready`) or dropped, its
//!    waker is never invoked again.
//! 3. **No stale registration.** A future that returned `Ready`
//!    (or was dropped) must not appear in the awaiter set; this is
//!    observed indirectly via property 2 — subsequent `set()` cycles
//!    must not bump the wake count of a consumed future.
//! 4. **No panics.** Random operation sequences must not panic.
//!
//! The operation grammar covers external `set` / `reset` / `try_wait`,
//! plus future lifecycle operations (`register`, `poll`, `drop`) and
//! position-aware reentrant waker actions.

#![cfg_attr(coverage_nightly, coverage(off))]
// Test code. Indices are produced by `live_slot_index`, which always
// returns an index that is in-bounds for the current `slots`
// vector. Indexing keeps the harness compact.
#![allow(
    clippy::indexing_slicing,
    reason = "indices are validated by live_slot_index before use"
)]
#![allow(
    clippy::arithmetic_side_effects,
    reason = "test code: bounded operation counts make overflow impossible"
)]

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
use std::thread::{self, ThreadId};

use proptest::collection::vec;
use proptest::prelude::*;

use crate::local_manual::LocalManualResetWaitFuture;
use crate::manual::ManualResetWaitFuture;
use crate::{LocalManualResetEvent, ManualResetEvent};

/// The maximum number of slots a sequence may produce. Bounded to
/// keep operation indices small enough that proptest can shrink
/// failures into short reproductions.
const MAX_SLOTS: usize = 8;

/// The maximum number of operations per sequence. Each operation
/// touches the system at most a few times; this keeps a single test
/// case fast while still exploring nontrivial interleavings.
pub(crate) const OPS_PER_SEQUENCE: usize = 24;

/// Abstraction over the manual-reset event variants we exercise.
///
/// The contract is the union of inherent methods both
/// `LocalManualResetEvent` and `ManualResetEvent` already expose;
/// implementations are one-line forwarders. Kept private to the
/// crate because it exists solely to deduplicate the test harness.
pub(crate) trait ManualEvent: 'static {
    type WaitFuture: Future<Output = ()>;

    fn create() -> Self;
    fn set(&self);
    fn reset(&self);
    fn try_wait(&self) -> bool;
    fn wait(&self) -> Self::WaitFuture;
}

impl ManualEvent for LocalManualResetEvent {
    type WaitFuture = LocalManualResetWaitFuture;

    fn create() -> Self {
        Self::boxed()
    }

    fn set(&self) {
        Self::set(self);
    }

    fn reset(&self) {
        Self::reset(self);
    }

    fn try_wait(&self) -> bool {
        Self::try_wait(self)
    }

    fn wait(&self) -> Self::WaitFuture {
        Self::wait(self)
    }
}

impl ManualEvent for ManualResetEvent {
    type WaitFuture = ManualResetWaitFuture;

    fn create() -> Self {
        Self::boxed()
    }

    fn set(&self) {
        Self::set(self);
    }

    fn reset(&self) {
        Self::reset(self);
    }

    fn try_wait(&self) -> bool {
        Self::try_wait(self)
    }

    fn wait(&self) -> Self::WaitFuture {
        Self::wait(self)
    }
}

/// What a reentrant waker does when invoked. The action runs
/// synchronously inside `wake()`, while the production code is in
/// the middle of an outer `set()` drain.
#[derive(Clone, Copy, Debug)]
pub(crate) enum WakerAction {
    /// Pure tracking waker. No side effects.
    None,
    /// Calls `event.set()` reentrantly. No-op while already set.
    Set,
    /// Calls `event.reset()` reentrantly. Closes the gate mid-drain.
    Reset,
    /// Drops the first other live future. Exercises the
    /// sibling-mutation reentrancy path from PR #141.
    DropFirstOther,
    /// Drops the last other live future. Mirrors `DropFirstOther`
    /// at the opposite end of the awaiter list.
    DropLastOther,
    /// Calls `event.reset()` then registers a fresh waiter with a
    /// noop waker. The fresh waiter belongs to the new generation
    /// and must remain `Pending` until the next effective `set()`.
    ResetThenRegister,
}

/// A single operation in a generated sequence. Indices into the slot
/// vector are reduced modulo the current live-slot count at runtime
/// so that operations always target some live slot when one exists.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Op {
    Set,
    Reset,
    TryWait,
    Register(WakerAction),
    Poll(usize),
    Drop(usize),
}

/// Logical lifecycle of a future stored in the harness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SlotState {
    /// Polled at least once and returned `Pending`; the inner
    /// awaiter is registered.
    Pending,
    /// An effective `set()` was observed while the slot was
    /// `Pending`. The next poll must return `Ready`.
    MustBeReady,
    /// The future returned `Ready` to the harness. The slot's
    /// waker must not be invoked again.
    Ready,
    /// The future was dropped. The slot's waker must not be
    /// invoked again.
    Dropped,
}

/// Tracking data behind a `Waker`. The pointer to this struct is
/// shared between the harness and the production code via the raw
/// waker vtable. The boxed action is invoked synchronously inside
/// `wake()`; it is set to a no-op for fresh waiters registered via
/// `ResetThenRegister`.
struct WakerTracker {
    /// Total wake invocations observed.
    wake_count: Cell<u32>,
    /// Upper bound on `wake_count`. Incremented by `1` whenever an
    /// effective `set()` finds the owning slot in the `Pending`
    /// state. After the slot enters `Ready` or `Dropped`, this
    /// bound is frozen alongside `wake_count`.
    expected_max_wakes: Cell<u32>,
    /// The reentrant action to run on wake. Wrapped in `RefCell`
    /// because installing the action requires a back-reference to
    /// the harness that did not exist at tracker creation time.
    action: RefCell<Box<dyn Fn()>>,
    /// Thread that created this tracker. Every vtable function
    /// asserts the current thread matches before touching the
    /// `!Sync` interior. This converts what would otherwise be UB
    /// (an `Rc`-backed waker observed from another thread) into a
    /// deterministic panic during testing. The harness itself is
    /// single-threaded by construction; the assertion is a guardrail
    /// against future modifications that might spawn tasks.
    creator_thread: ThreadId,
}

impl WakerTracker {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            wake_count: Cell::new(0),
            expected_max_wakes: Cell::new(0),
            action: RefCell::new(Box::new(|| {})),
            creator_thread: thread::current().id(),
        })
    }

    fn install_action(self: &Rc<Self>, action: Box<dyn Fn()>) {
        *self.action.borrow_mut() = action;
    }
}

/// Tracking record for a single future created by `Register`.
struct Slot<F> {
    future: Option<Pin<Box<F>>>,
    tracker: Rc<WakerTracker>,
    state: SlotState,
}

/// The harness state. All fields use narrow interior mutability so
/// that reentrant waker actions can mutate the harness without
/// triggering `RefCell` borrow conflicts with the outer operation
/// runner.
struct Harness<E: ManualEvent> {
    event: E,
    /// Mirror of `event.try_wait()`. Updated by the harness before
    /// the corresponding event call so that reentrant actions see a
    /// consistent model.
    is_set: Cell<bool>,
    slots: RefCell<Vec<Slot<E::WaitFuture>>>,
}

impl<E: ManualEvent> Harness<E> {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            event: E::create(),
            is_set: Cell::new(false),
            slots: RefCell::new(Vec::new()),
        })
    }

    /// Performs an effective `set()` from outside any reentrant
    /// context. Snapshots the current Pending slots, increments
    /// their wake budgets, transitions them to `MustBeReady`, then
    /// invokes `event.set()`.
    ///
    /// If the event is already set, this is a no-op (matches
    /// production behavior).
    fn perform_set(self: &Rc<Self>) {
        if self.is_set.get() {
            // No effective transition; production code short-circuits.
            self.event.set();
            return;
        }
        self.is_set.set(true);
        {
            let slots = self.slots.borrow();
            for slot in slots.iter() {
                if slot.state == SlotState::Pending {
                    let tracker = &slot.tracker;
                    tracker
                        .expected_max_wakes
                        .set(tracker.expected_max_wakes.get().saturating_add(1));
                }
            }
            // Transition states in a separate pass to keep the borrow
            // narrowly scoped and obvious.
        }
        {
            let mut slots = self.slots.borrow_mut();
            for slot in slots.iter_mut() {
                if slot.state == SlotState::Pending {
                    slot.state = SlotState::MustBeReady;
                }
            }
        }
        self.event.set();
    }

    fn perform_reset(self: &Rc<Self>) {
        self.is_set.set(false);
        self.event.reset();
    }

    fn perform_try_wait(self: &Rc<Self>) -> bool {
        let observed = self.event.try_wait();
        // `try_wait` is a pure read and must agree with our mirror.
        assert_eq!(observed, self.is_set.get());
        observed
    }

    /// Registers a new future with a tracker carrying `action`.
    /// Polls it once and records the resulting state.
    fn perform_register(self: &Rc<Self>, action: WakerAction) {
        let tracker = WakerTracker::new();
        self.install_action(&tracker, action);

        let mut future = Box::pin(self.event.wait());
        let waker = make_waker(&tracker);
        let mut cx = Context::from_waker(&waker);

        let was_set = self.is_set.get();
        let poll_result = future.as_mut().poll(&mut cx);

        let state = match poll_result {
            Poll::Ready(()) => {
                // The contract: a Ready poll on a fresh future is
                // only valid when the event is set.
                assert!(was_set, "fresh future returned Ready while event was unset");
                SlotState::Ready
            }
            Poll::Pending => {
                assert!(
                    !was_set,
                    "fresh future returned Pending while event was set"
                );
                SlotState::Pending
            }
        };

        // If the future returned Ready immediately, we never store
        // it back into the slot — it is dropped at end of this scope.
        // Keep the slot for tracking even so, because the tracker
        // continues to exist and must not see further wakes.
        let future_to_store = if matches!(state, SlotState::Pending) {
            Some(future)
        } else {
            None
        };
        self.slots.borrow_mut().push(Slot {
            future: future_to_store,
            tracker,
            state,
        });
    }

    /// Installs the wake action on `tracker`. The action runs inside
    /// `wake()`. It must drop any harness borrows before invoking
    /// event methods, because nested `event.set()` calls may
    /// re-enter the wake action chain and need their own borrows.
    ///
    /// The closure captures a [`Weak`](std::rc::Weak) reference to
    /// the harness to avoid a reference cycle: `Harness → slots →
    /// Slot → tracker → action → Rc<Harness>`. Upgrades succeed for
    /// the entire duration of `run_sequence`, which holds the only
    /// strong reference; after that, no further wakes can fire.
    fn install_action(self: &Rc<Self>, tracker: &Rc<WakerTracker>, action: WakerAction) {
        let weak = Rc::downgrade(self);
        let action_fn: Box<dyn Fn()> = match action {
            WakerAction::None => Box::new(|| {}),
            WakerAction::Set => Box::new(move || {
                let harness = weak.upgrade().unwrap();
                harness.perform_set();
            }),
            WakerAction::Reset => Box::new(move || {
                let harness = weak.upgrade().unwrap();
                harness.perform_reset();
            }),
            WakerAction::DropFirstOther => Box::new(move || {
                let harness = weak.upgrade().unwrap();
                harness.drop_other(DropEnd::First);
            }),
            WakerAction::DropLastOther => Box::new(move || {
                let harness = weak.upgrade().unwrap();
                harness.drop_other(DropEnd::Last);
            }),
            WakerAction::ResetThenRegister => Box::new(move || {
                let harness = weak.upgrade().unwrap();
                harness.perform_reset();
                harness.perform_register(WakerAction::None);
            }),
        };
        tracker.install_action(action_fn);
    }

    /// Drops the first or last live future (other than one already
    /// being woken). Used by the `DropFirstOther` / `DropLastOther`
    /// reentrant actions to exercise sibling-mutation paths.
    fn drop_other(self: &Rc<Self>, end: DropEnd) {
        // Identify the target slot without holding the borrow across
        // the drop, because the future's destructor calls back into
        // the event (and could in principle re-enter the harness).
        let target = {
            let slots = self.slots.borrow();
            let candidates = slots
                .iter()
                .enumerate()
                .filter(|(_, slot)| slot.future.is_some());
            match end {
                DropEnd::First => candidates.map(|(idx, _)| idx).next(),
                DropEnd::Last => candidates.map(|(idx, _)| idx).next_back(),
            }
        };
        if let Some(idx) = target {
            // Mark the slot Dropped under the same borrow that takes
            // the future, so a hypothetical reentrant observation
            // during the destructor would see a consistent state
            // (Dropped + no future) rather than the transient
            // Pending + no future combination.
            let future = {
                let mut slots = self.slots.borrow_mut();
                let future = slots[idx].future.take();
                slots[idx].state = SlotState::Dropped;
                future
            };
            drop(future);
        }
    }

    /// Polls the slot at `live_index`-th live position. Validates the
    /// poll result against the modeled state. A live slot is one
    /// whose future is still owned by the harness.
    fn perform_poll(self: &Rc<Self>, live_index: usize) {
        let target = self.live_slot_index(live_index);
        let Some(idx) = target else { return };

        // Take ownership of the future temporarily so the slot
        // borrow can be released before we poll (poll may invoke a
        // reentrant action that re-borrows slots).
        let mut future = self.slots.borrow_mut()[idx]
            .future
            .take()
            .expect("live slot must have a future");
        let tracker = Rc::clone(&self.slots.borrow()[idx].tracker);
        let must_be_ready = self.slots.borrow()[idx].state == SlotState::MustBeReady;

        let waker = make_waker(&tracker);
        let mut cx = Context::from_waker(&waker);
        let result = future.as_mut().poll(&mut cx);

        match result {
            Poll::Ready(()) => {
                // Mark the slot Ready before dropping the future so
                // the destructor cannot observe a transient state
                // where the future is gone but the slot still says
                // Pending / MustBeReady.
                self.slots.borrow_mut()[idx].state = SlotState::Ready;
                drop(future);
            }
            Poll::Pending => {
                assert!(
                    !must_be_ready,
                    "future in MustBeReady state returned Pending"
                );
                self.slots.borrow_mut()[idx].future = Some(future);
            }
        }
    }

    /// Drops the future at `live_index`-th live position.
    fn perform_drop(self: &Rc<Self>, live_index: usize) {
        let target = self.live_slot_index(live_index);
        let Some(idx) = target else { return };
        // Same defensive pattern as `drop_other`: update the slot
        // state before the future destructor runs so any reentrant
        // observer sees a consistent slot.
        let future = {
            let mut slots = self.slots.borrow_mut();
            let future = slots[idx].future.take();
            slots[idx].state = SlotState::Dropped;
            future
        };
        drop(future);
    }

    fn live_slot_index(&self, live_index: usize) -> Option<usize> {
        let slots = self.slots.borrow();
        let live: Vec<usize> = slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.future.is_some())
            .map(|(idx, _)| idx)
            .collect();
        if live.is_empty() {
            None
        } else {
            Some(live[live_index % live.len()])
        }
    }

    /// Validates the per-tracker invariants. Called after every op.
    fn check_invariants(&self) {
        let slots = self.slots.borrow();
        for (idx, slot) in slots.iter().enumerate() {
            let wake_count = slot.tracker.wake_count.get();
            let budget = slot.tracker.expected_max_wakes.get();
            assert!(
                wake_count <= budget,
                "slot {idx}: wake_count {wake_count} exceeds budget {budget} \
                 (state {:?})",
                slot.state,
            );
            // `set()` is synchronous, so by the time control returns
            // to the harness every snapshot waiter must have been
            // invoked. A `MustBeReady` slot whose `wake_count` lags
            // behind its frozen budget is a lost notification — even
            // if the sequence never polls the future again. This
            // catches "no lost notifications" violations directly,
            // without needing a follow-up `Poll` op.
            if slot.state == SlotState::MustBeReady {
                assert_eq!(
                    wake_count, budget,
                    "slot {idx}: MustBeReady but wake_count {wake_count} < budget {budget} \
                     (lost notification: set() returned without invoking this waker)",
                );
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum DropEnd {
    First,
    Last,
}

/// Reentrant `Rc<WakerTracker>`-backed `Waker`. The raw pointer
/// borrows from a clone of the `Rc`, kept alive by leaking the
/// clone for the duration of every outstanding `Waker`. The leak is
/// recovered when the waker is dropped via `drop_fn`.
///
/// `WakerTracker` is `!Send`/`!Sync` because it holds `Cell` and
/// `RefCell`. The `Waker` type is `Send + Sync` by trait, so this
/// constructor relies on a runtime guardrail: every vtable function
/// asserts that the current thread matches `tracker.creator_thread`.
/// Single-threaded misuse panics deterministically; any future change
/// that accidentally moves a waker off-thread will be caught instead
/// of becoming UB.
fn make_waker(tracker: &Rc<WakerTracker>) -> Waker {
    // Convert a strong reference into a raw pointer that we own.
    // The matching `Rc::from_raw` happens in `drop_fn`.
    let cloned = Rc::clone(tracker);
    let raw = Rc::into_raw(cloned);
    let data: *const () = raw.cast::<()>();

    // SAFETY: Validity — `data` was just produced from `Rc::into_raw`
    // and owns a strong count; the vtable maintains that count
    // (`clone_fn` increments, `drop_fn` decrements), so the pointee
    // stays alive for the duration of every outstanding `Waker`.
    // Aliasing — vtable functions only ever construct shared
    // `&WakerTracker` references, never `&mut`, so reentrant wakes
    // may freely create additional shared references without
    // violating Rust's aliasing rules. The `!Sync` interior (`Cell`
    // and `RefCell`) is protected by `assert_creator_thread`, which
    // pins every vtable invocation to the thread that created the
    // tracker — preventing cross-thread observation of the
    // single-threaded interior mutability primitives.
    unsafe { Waker::from_raw(RawWaker::new(data, &VTABLE)) }
}

fn clone_fn(data: *const ()) -> RawWaker {
    assert_creator_thread(data);
    // SAFETY: `data` was produced by `Rc::into_raw` in `make_waker`
    // or by a prior `clone_fn` invocation; either way the pointer
    // owns a strong count. `Rc::increment_strong_count` requires
    // such a pointer.
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
    // SAFETY: Validity — `data` owns a strong count on a
    // `WakerTracker` (see `clone_fn` / `make_waker`), so the pointee
    // is alive for the duration of this call. Aliasing — we only
    // construct a shared `&WakerTracker`; reentrant invocations may
    // construct further shared references, which is sound. The
    // `!Sync` interior (`Cell` and `RefCell`) is safe to touch here
    // because `assert_creator_thread` confines this call to the
    // tracker's creator thread, so no cross-thread observation
    // occurs.
    let tracker = unsafe { &*data.cast::<WakerTracker>() };
    tracker
        .wake_count
        .set(tracker.wake_count.get().saturating_add(1));
    // The action may re-enter the harness. The harness's
    // contract is that no harness borrow is held while event
    // methods are invoked, so the action can freely borrow.
    let action = tracker.action.borrow();
    (action)();
}

fn drop_fn(data: *const ()) {
    assert_creator_thread(data);
    // SAFETY: `data` was produced by `Rc::into_raw` in `make_waker`
    // or by a `clone_fn` increment; reconstructing the `Rc` here
    // decrements the strong count and (eventually) frees the
    // `WakerTracker` when no references remain.
    unsafe {
        drop(Rc::<WakerTracker>::from_raw(data.cast::<WakerTracker>()));
    }
}

/// Asserts that the calling thread is the one that created the
/// tracker behind `data`. Panics otherwise. Reading the
/// `creator_thread` field is safe from any thread because `ThreadId`
/// is `Copy + Sync`, and the field is never mutated after
/// construction.
fn assert_creator_thread(data: *const ()) {
    // SAFETY: Validity — `data` owns a strong count on a
    // `WakerTracker` (see `make_waker` / `clone_fn`), so the
    // pointee is alive. Aliasing — we only construct a shared
    // `&WakerTracker` and only read the `Sync` `ThreadId` field
    // (never the `!Sync` `Cell`/`RefCell` fields), so this is sound
    // even from a thread other than the creator and even when other
    // shared references to the same tracker coexist.
    let tracker = unsafe { &*data.cast::<WakerTracker>() };
    assert_eq!(
        tracker.creator_thread,
        thread::current().id(),
        "proptest waker accessed from a thread other than its creator",
    );
}

static VTABLE: RawWakerVTable = RawWakerVTable::new(clone_fn, wake_fn, wake_by_ref_fn, drop_fn);

fn waker_action_strategy() -> impl Strategy<Value = WakerAction> {
    prop_oneof![
        4 => Just(WakerAction::None),
        1 => Just(WakerAction::Set),
        1 => Just(WakerAction::Reset),
        1 => Just(WakerAction::DropFirstOther),
        1 => Just(WakerAction::DropLastOther),
        1 => Just(WakerAction::ResetThenRegister),
    ]
}

pub(crate) fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        2 => Just(Op::Set),
        2 => Just(Op::Reset),
        1 => Just(Op::TryWait),
        4 => waker_action_strategy().prop_map(Op::Register),
        3 => (0_usize..MAX_SLOTS).prop_map(Op::Poll),
        2 => (0_usize..MAX_SLOTS).prop_map(Op::Drop),
    ]
}

pub(crate) fn ops_strategy() -> impl Strategy<Value = Vec<Op>> {
    vec(op_strategy(), 1..=OPS_PER_SEQUENCE)
}

/// Executes one operation sequence end-to-end against a manual-reset
/// event of type `E`. On a property violation, this function panics,
/// which proptest turns into a shrunk reproduction case.
pub(crate) fn run_sequence<E: ManualEvent>(ops: &[Op]) {
    let harness = Harness::<E>::new();
    for op in ops {
        match *op {
            Op::Set => harness.perform_set(),
            Op::Reset => harness.perform_reset(),
            Op::TryWait => {
                _ = harness.perform_try_wait();
            }
            Op::Register(action) => {
                if harness.slots.borrow().len() < MAX_SLOTS {
                    harness.perform_register(action);
                }
            }
            Op::Poll(live_index) => harness.perform_poll(live_index),
            Op::Drop(live_index) => harness.perform_drop(live_index),
        }
        harness.check_invariants();
    }
    // Final cleanup: drop everything explicitly so the destructor
    // path runs while we still hold the harness, then validate
    // invariants one last time.
    {
        let mut slots = harness.slots.borrow_mut();
        for slot in slots.iter_mut() {
            if let Some(future) = slot.future.take() {
                drop(future);
                slot.state = SlotState::Dropped;
            }
        }
    }
    harness.check_invariants();
}
