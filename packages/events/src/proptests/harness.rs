//! Harness that drives an [`Op`] sequence against a manual-reset event
//! and validates it against a reference [`Model`].
//!
//! The harness owns two parallel structures: a vector of [`Slot`]s
//! holding live [`Future`]s plus their [`WakerTracker`]s (the SUT side),
//! and a [`Model`] mirroring the *expected* state. Every operation
//! updates both, then [`Harness::check_invariants`] verifies the SUT
//! against the model.
//!
//! Slot indices in [`Harness::slots`] are kept aligned with
//! [`Model::add_slot`] return values via the lifecycle helpers
//! ([`Harness::add_slot`], [`Harness::mark_ready`], and
//! [`Harness::take_future_and_mark_dropped`]). The structural alignment
//! itself is verified by [`Harness::check_invariants`].

// Test code. Indices are produced by `live_slot_index`, which always
// returns an index in-bounds for the current `slots` vector. Indexing
// keeps the harness compact.
#![allow(
    clippy::indexing_slicing,
    reason = "indices are validated by live_slot_index before use"
)]
#![allow(
    clippy::arithmetic_side_effects,
    reason = "test code: bounded operation counts make overflow impossible"
)]

use std::cell::RefCell;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll};

use crate::proptests::grammar::{MAX_SLOTS, Op, WakerAction};
use crate::proptests::manual_event::ManualEvent;
use crate::proptests::model::{Model, SlotState};
use crate::proptests::waker_tracker::{WakerTracker, make_waker};

/// SUT-side record for a single registered future.
///
/// The lifecycle (`Pending`/`MustBeReady`/`Ready`/`Dropped`) lives in
/// [`Model`]; this struct holds only the live `Future` and the waker
/// tracker.
struct Slot<F> {
    future: Option<Pin<Box<F>>>,
    tracker: Rc<WakerTracker>,
}

/// Drives an operation sequence and checks invariants.
///
/// Field interior mutability is intentionally narrow: reentrant waker
/// actions may re-borrow the harness, so every borrow scope must be
/// kept short and obvious.
pub(super) struct Harness<E: ManualEvent> {
    event: E,
    /// SUT-side slot records. Index `i` corresponds to model slot `i`.
    slots: RefCell<Vec<Slot<E::WaitFuture>>>,
    /// Reference model. Drives invariant assertions.
    model: Model,
}

impl<E: ManualEvent> Harness<E> {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            event: E::create(),
            slots: RefCell::new(Vec::new()),
            model: Model::new(),
        })
    }

    // ----- Operations -------------------------------------------------

    /// Performs an `event.set()` while keeping the model in sync.
    ///
    /// If the event was already set, this is a no-op transition: the
    /// model is unchanged and the underlying `event.set()` call is a
    /// production-defined no-op. Otherwise, the model's `set_effective`
    /// is invoked *before* the SUT call so reentrant wakers observing
    /// the model see the post-set view.
    fn perform_set(self: &Rc<Self>) {
        if self.model.is_set() {
            self.event.set();
            return;
        }
        self.model.set_effective();
        self.event.set();
    }

    fn perform_reset(self: &Rc<Self>) {
        self.model.reset();
        self.event.reset();
    }

    fn perform_try_wait(self: &Rc<Self>) -> bool {
        let observed = self.event.try_wait();
        // try_wait is a pure read; it must agree with the model.
        assert_eq!(
            observed,
            self.model.is_set(),
            "try_wait diverged from model: SUT={observed}, model={}",
            self.model.is_set(),
        );
        observed
    }

    /// Registers a new future and polls it once.
    fn perform_register(self: &Rc<Self>, action: WakerAction) {
        let tracker = WakerTracker::new();
        self.install_action(&tracker, action);

        let mut future = Box::pin(self.event.wait());
        let waker = make_waker(&tracker);
        let mut cx = Context::from_waker(&waker);

        let was_set = self.model.is_set();
        let poll_result = future.as_mut().poll(&mut cx);

        let (initial_state, future_to_store) = match poll_result {
            Poll::Ready(()) => {
                // Per-op transition postcondition (invariant #1
                // perspective on fresh polls): a fresh future may
                // return Ready only when the event is set.
                assert!(was_set, "fresh future returned Ready while event was unset");
                (SlotState::Ready, None)
            }
            Poll::Pending => {
                // Per-op transition postcondition: a fresh future may
                // return Pending only when the event is unset.
                assert!(
                    !was_set,
                    "fresh future returned Pending while event was set"
                );
                (SlotState::Pending, Some(future))
            }
        };
        self.add_slot(future_to_store, tracker, initial_state);
    }

    /// Polls the slot at the `live_index`-th live position. A "live"
    /// slot is one whose future is still owned by the harness.
    fn perform_poll(self: &Rc<Self>, live_index: usize) {
        let Some(idx) = self.live_slot_index(live_index) else {
            return;
        };

        // Temporarily take ownership of the future so the slot borrow
        // can be released before polling. Polling may invoke a
        // reentrant action that re-borrows the slots vector.
        let mut future = self.take_future(idx);
        let tracker = Rc::clone(&self.slots.borrow()[idx].tracker);
        let must_be_ready = self.model.slot_state(idx) == SlotState::MustBeReady;

        let waker = make_waker(&tracker);
        let mut cx = Context::from_waker(&waker);
        let result = future.as_mut().poll(&mut cx);

        match result {
            Poll::Ready(()) => {
                // Mark the slot Ready *before* dropping the future so
                // any reentrant observer triggered by the destructor
                // sees a consistent state (Ready, no future) rather
                // than (Pending, no future).
                self.mark_ready(idx);
                drop(future);
            }
            Poll::Pending => {
                // Per-op transition postcondition (invariant #1, "no
                // lost notifications"): a slot that the model has
                // marked MustBeReady must observe Ready on its next
                // poll.
                assert!(
                    !must_be_ready,
                    "future in MustBeReady state returned Pending"
                );
                self.restore_future(idx, future);
            }
        }
    }

    /// Drops the future at the `live_index`-th live position.
    fn perform_drop(self: &Rc<Self>, live_index: usize) {
        let Some(idx) = self.live_slot_index(live_index) else {
            return;
        };
        let future = self.take_future_and_mark_dropped(idx);
        drop(future);
    }

    // ----- Slot lifecycle helpers (keep SUT and model aligned) -------

    /// Atomically appends a new SUT slot and the matching model slot.
    /// Guarantees `slots[i]` corresponds to model slot `i`.
    fn add_slot(
        &self,
        future: Option<Pin<Box<E::WaitFuture>>>,
        tracker: Rc<WakerTracker>,
        initial_state: SlotState,
    ) {
        let mut slots = self.slots.borrow_mut();
        let model_idx = self.model.add_slot(initial_state);
        slots.push(Slot { future, tracker });
        debug_assert_eq!(
            slots.len() - 1,
            model_idx,
            "slot index alignment broken on add_slot",
        );
    }

    /// Removes the live future from `slot[idx]` without touching the
    /// model. Used to poll the future outside of any harness borrow.
    fn take_future(&self, idx: usize) -> Pin<Box<E::WaitFuture>> {
        self.slots.borrow_mut()[idx]
            .future
            .take()
            .expect("take_future called on a slot whose future is already gone")
    }

    /// Restores a previously-taken future to `slot[idx]`. The model
    /// state is unchanged.
    fn restore_future(&self, idx: usize, future: Pin<Box<E::WaitFuture>>) {
        self.slots.borrow_mut()[idx].future = Some(future);
    }

    /// Transitions the model slot at `idx` to [`SlotState::Ready`].
    /// The caller is responsible for ensuring the future is gone (do
    /// not call [`Self::restore_future`] after this). The model
    /// freezes the budget at the slot's current `wake_count` so any
    /// future wake on this terminal slot is caught.
    fn mark_ready(&self, idx: usize) {
        let wake_count = self.slots.borrow()[idx].tracker.wake_count.get();
        self.model.mark_ready(idx, wake_count);
    }

    /// Atomically takes the future out of `slot[idx]` and transitions
    /// the model to [`SlotState::Dropped`]. Done under one borrow so
    /// any reentrant observer triggered by the destructor sees a
    /// consistent (Dropped, no future) pair. The model freezes the
    /// budget at the slot's current `wake_count` for the same reason
    /// as [`Self::mark_ready`].
    fn take_future_and_mark_dropped(&self, idx: usize) -> Option<Pin<Box<E::WaitFuture>>> {
        let mut slots = self.slots.borrow_mut();
        let future = slots[idx].future.take();
        let wake_count = slots[idx].tracker.wake_count.get();
        self.model.mark_dropped(idx, wake_count);
        future
    }

    // ----- Reentrant helpers -----------------------------------------

    /// Installs the wake action on `tracker`. The action runs inside
    /// `wake()`. It must drop any harness borrows before invoking
    /// event methods, because nested `event.set()` calls may re-enter
    /// the wake action chain and need their own borrows.
    ///
    /// The closure captures a [`Weak`](std::rc::Weak) reference to the
    /// harness to avoid a reference cycle: `Harness → slots → Slot →
    /// tracker → action → Rc<Harness>`. Upgrades succeed for the
    /// entire duration of [`run_sequence`], which holds the only
    /// strong reference; after that, no further wakes can fire.
    ///
    /// `DropFirstOther` / `DropLastOther` capture a separate
    /// `Weak<WakerTracker>` for the slot they are installed on so
    /// `drop_other` can exclude it — preserving the "other" in the
    /// action name even when the harness draws the same waker as both
    /// invoker and candidate.
    fn install_action(self: &Rc<Self>, tracker: &Rc<WakerTracker>, action: WakerAction) {
        let weak_harness = Rc::downgrade(self);
        let action_fn: Box<dyn Fn()> = match action {
            WakerAction::None => Box::new(|| {}),
            WakerAction::Set => Box::new(move || {
                let harness = weak_harness.upgrade().unwrap();
                harness.perform_set();
            }),
            WakerAction::Reset => Box::new(move || {
                let harness = weak_harness.upgrade().unwrap();
                harness.perform_reset();
            }),
            WakerAction::DropFirstOther => {
                let weak_tracker = Rc::downgrade(tracker);
                Box::new(move || {
                    let harness = weak_harness.upgrade().unwrap();
                    let current = weak_tracker.upgrade();
                    harness.drop_other(DropEnd::First, current.as_ref());
                })
            }
            WakerAction::DropLastOther => {
                let weak_tracker = Rc::downgrade(tracker);
                Box::new(move || {
                    let harness = weak_harness.upgrade().unwrap();
                    let current = weak_tracker.upgrade();
                    harness.drop_other(DropEnd::Last, current.as_ref());
                })
            }
            WakerAction::ResetThenRegister => Box::new(move || {
                let harness = weak_harness.upgrade().unwrap();
                harness.perform_reset();
                harness.perform_register(WakerAction::None);
            }),
        };
        tracker.install_action(action_fn);
    }

    /// Drops the first or last live future *other than* the one whose
    /// waker is currently firing. Used by the `DropFirstOther` /
    /// `DropLastOther` actions to exercise sibling-mutation paths (the
    /// bug shape from PR #141).
    ///
    /// `current_tracker` identifies the running slot's tracker so it
    /// can be excluded from candidates via [`Rc::ptr_eq`]. If the
    /// `Weak` failed to upgrade (which should not happen during a
    /// live wake), no slot is excluded.
    fn drop_other(self: &Rc<Self>, end: DropEnd, current_tracker: Option<&Rc<WakerTracker>>) {
        // Identify the target slot without holding the borrow across
        // the drop, because the future's destructor calls back into
        // the event (and could in principle re-enter the harness).
        let target = {
            let slots = self.slots.borrow();
            let candidates = slots.iter().enumerate().filter(|(_, slot)| {
                slot.future.is_some()
                    && current_tracker.is_none_or(|cur| !Rc::ptr_eq(&slot.tracker, cur))
            });
            match end {
                DropEnd::First => candidates.map(|(idx, _)| idx).next(),
                DropEnd::Last => candidates.map(|(idx, _)| idx).next_back(),
            }
        };
        if let Some(idx) = target {
            let future = self.take_future_and_mark_dropped(idx);
            drop(future);
        }
    }

    /// Returns the SUT-side slot index corresponding to the
    /// `live_index`-th *live* slot (one whose future is still owned),
    /// or `None` if no slot is live. `live_index` is reduced modulo
    /// the live count.
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

    // ----- Invariants -------------------------------------------------

    /// Runs every named invariant check.
    ///
    /// Invariant #4 ("no panics") is implicit: any check that panics
    /// surfaces as a proptest failure, and so does any panic from the
    /// SUT itself.
    fn check_invariants(&self) {
        // Structural invariant: the SUT and the model must remain
        // index-aligned for the per-slot invariants below to be
        // meaningful. Broken alignment indicates a missing lifecycle
        // helper call somewhere in the harness.
        assert_eq!(
            self.slots.borrow().len(),
            self.model.num_slots(),
            "slot index alignment broken: SUT and model slot counts diverged",
        );

        self.check_invariant_no_lost_notifications();
        self.check_invariant_no_stale_registration();
        self.check_invariant_wake_budget();
    }

    /// Invariant #1: no lost notifications.
    ///
    /// `event.set()` is synchronous, so by the time control returns to
    /// the harness every snapshot waiter (a slot transitioned to
    /// [`SlotState::MustBeReady`]) must have observed at least its
    /// expected number of wakes. A [`SlotState::MustBeReady`] slot
    /// whose tracker has fewer wakes than the budget is a *lost
    /// notification* even if the sequence never polls the future
    /// again. This is a *lower-bound* check: any over-wake is the
    /// concern of [`Self::check_invariant_wake_budget`].
    fn check_invariant_no_lost_notifications(&self) {
        let slots = self.slots.borrow();
        for (idx, slot) in slots.iter().enumerate() {
            if self.model.slot_state(idx) == SlotState::MustBeReady {
                let wake_count = slot.tracker.wake_count.get();
                let budget = self.model.slot_budget(idx);
                assert!(
                    wake_count >= budget,
                    "slot {idx}: MustBeReady but wake_count {wake_count} < budget {budget} \
                     (lost notification: set() returned without invoking this waker)",
                );
            }
        }
    }

    /// Invariant #2: wake budget.
    ///
    /// No waker may be invoked more times than the model permits. The
    /// budget grows only when an effective `set()` finds the owning
    /// slot in [`SlotState::Pending`]. Any over-budget wake — whether
    /// it is a duplicate during a single `set()` drain or a stale wake
    /// after the slot has been consumed or dropped — is caught here.
    fn check_invariant_wake_budget(&self) {
        let slots = self.slots.borrow();
        for (idx, slot) in slots.iter().enumerate() {
            let wake_count = slot.tracker.wake_count.get();
            let budget = self.model.slot_budget(idx);
            assert!(
                wake_count <= budget,
                "slot {idx}: wake_count {wake_count} exceeds budget {budget} \
                 (state {:?})",
                self.model.slot_state(idx),
            );
        }
    }

    /// Invariant #3: no stale registration.
    ///
    /// Once a slot reaches a terminal state ([`SlotState::Ready`] or
    /// [`SlotState::Dropped`]), its waker must never fire again. The
    /// model freezes the budget at the slot's observed `wake_count` at
    /// the moment of transition, so any subsequent waker invocation
    /// would push `wake_count` above the frozen budget. This check
    /// surfaces such stale wakes with a precise message before the
    /// more generic wake-budget invariant flags the same condition.
    fn check_invariant_no_stale_registration(&self) {
        let slots = self.slots.borrow();
        for (idx, slot) in slots.iter().enumerate() {
            let state = self.model.slot_state(idx);
            if matches!(state, SlotState::Ready | SlotState::Dropped) {
                let wake_count = slot.tracker.wake_count.get();
                let budget = self.model.slot_budget(idx);
                assert_eq!(
                    wake_count, budget,
                    "slot {idx}: terminal state {state:?} but wake_count {wake_count} \
                     diverges from frozen budget {budget} \
                     (stale registration: waker fired after slot reached terminal state)",
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

/// Executes one operation sequence end-to-end against a manual-reset
/// event of type `E`. On any invariant violation, this function panics,
/// which proptest turns into a shrunk reproduction case.
pub(super) fn run_sequence<E: ManualEvent>(ops: &[Op]) {
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
    // Final cleanup: drop every remaining live future so its destructor
    // path runs while we still hold the harness, then validate the
    // invariants one last time. Any stale wake delivered during the
    // teardown destructors will be caught.
    let live_indices: Vec<usize> = {
        let slots = harness.slots.borrow();
        slots
            .iter()
            .enumerate()
            .filter(|(_, slot)| slot.future.is_some())
            .map(|(idx, _)| idx)
            .collect()
    };
    for idx in live_indices {
        let future = harness.take_future_and_mark_dropped(idx);
        drop(future);
    }
    harness.check_invariants();

    // Latent stale-registration smoke check: every slot is now in a
    // terminal state with its budget frozen at its observed wake
    // count. If a buggy SUT left a dropped/ready waker registered, a
    // fresh `reset(); set();` cycle would fire it, pushing
    // `wake_count` above the frozen budget. The model treats this as
    // a no-op for the slots (none are `Pending`), so any wake at all
    // is a stale-registration failure.
    harness.perform_reset();
    harness.perform_set();
    harness.check_invariants();
}
