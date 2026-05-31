//! Reference model of the expected event-under-test state.
//!
//! The model is **independent of the SUT (System Under Test) mechanics**:
//! it captures only what *should* be true according to the manual-reset
//! event contract, expressed as a small per-slot state machine plus a
//! mirror of `is_set`. The harness drives the model and the SUT in
//! lockstep and then asserts they agree.
//!
//! Model state is mutated only through the transition methods defined on
//! [`Model`] — never directly by external code. This makes the reference
//! semantics auditable in one place.

// Test code. Indices passed in by the harness come from
// `live_slot_index` (validated to be in-bounds for the parallel SUT
// vector); the model maintains the same length, so the same indices
// are in-bounds here.
#![allow(
    clippy::indexing_slicing,
    reason = "indices originate from live_slot_index and are in-bounds for the parallel model vector"
)]

use std::cell::{Cell, RefCell};

/// Lifecycle phase of a registered future from the model's perspective.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SlotState {
    /// Polled at least once and returned `Pending`; the inner awaiter is
    /// registered with the production code.
    Pending,
    /// An effective `set()` was observed while the slot was `Pending`.
    /// The next poll must return `Ready`.
    MustBeReady,
    /// The future returned `Ready` to the harness. The slot's waker must
    /// not be invoked again.
    Ready,
    /// The future was dropped. The slot's waker must not be invoked
    /// again.
    Dropped,
}

/// Per-slot model state.
#[derive(Clone, Copy, Debug)]
pub(super) struct SlotModel {
    pub(super) state: SlotState,
    /// Upper bound on the matching [`WakerTracker::wake_count`].
    /// Incremented by `1` whenever an effective `set()` finds this slot
    /// in [`SlotState::Pending`]. When the slot transitions to a
    /// terminal state ([`SlotState::Ready`] or [`SlotState::Dropped`]),
    /// the budget is frozen at the SUT-observed `wake_count` at the
    /// moment of transition so that any subsequent (stale) wake
    /// invocation pushes `wake_count` above the frozen budget.
    ///
    /// [`WakerTracker::wake_count`]: super::waker_tracker::WakerTracker::wake_count
    pub(super) expected_max_wakes: u32,
}

/// Reference model of the entire event-under-test.
///
/// All transitions happen through the methods on this type. The
/// [`Harness`](super::harness::Harness) reads the model after every
/// operation to assert invariants against the SUT.
pub(super) struct Model {
    /// Mirror of `event.try_wait()`.
    is_set: Cell<bool>,
    /// Per-slot model state. Indexed in parallel with
    /// [`Harness::slots`](super::harness::Harness::slots).
    slots: RefCell<Vec<SlotModel>>,
}

impl Model {
    pub(super) fn new() -> Self {
        Self {
            is_set: Cell::new(false),
            slots: RefCell::new(Vec::new()),
        }
    }

    pub(super) fn is_set(&self) -> bool {
        self.is_set.get()
    }

    pub(super) fn num_slots(&self) -> usize {
        self.slots.borrow().len()
    }

    pub(super) fn slot_state(&self, idx: usize) -> SlotState {
        self.slots.borrow()[idx].state
    }

    pub(super) fn slot_budget(&self, idx: usize) -> u32 {
        self.slots.borrow()[idx].expected_max_wakes
    }

    /// Effective `set()` transition.
    ///
    /// Called when the event was not previously set. Flips `is_set`,
    /// then for every slot in [`SlotState::Pending`]:
    ///
    /// * Increments `expected_max_wakes` by `1` (this `set()` will fire
    ///   the slot's waker exactly once).
    /// * Transitions the slot to [`SlotState::MustBeReady`] (its next
    ///   poll must return `Ready`).
    ///
    /// The actual `event.set()` call is made by the harness *after* this
    /// model transition, so by the time control returns to the harness
    /// every snapshot waiter must have been invoked synchronously.
    pub(super) fn set_effective(&self) {
        self.is_set.set(true);
        let mut slots = self.slots.borrow_mut();
        for slot in slots.iter_mut() {
            if slot.state == SlotState::Pending {
                slot.expected_max_wakes = slot
                    .expected_max_wakes
                    .checked_add(1)
                    .expect("wake budget overflow: too many effective set() calls in one test");
                slot.state = SlotState::MustBeReady;
            }
        }
    }

    /// `reset()` transition.
    ///
    /// Only flips `is_set`. Per-slot state is intentionally unchanged: a
    /// reset after set does not retract notifications already issued, so
    /// [`SlotState::MustBeReady`] slots remain `MustBeReady` through
    /// reset cycles.
    pub(super) fn reset(&self) {
        self.is_set.set(false);
    }

    /// Registers a new slot in `initial_state` with budget `0`.
    ///
    /// Returns the new slot's index, which the caller must align with
    /// the matching SUT slot.
    pub(super) fn add_slot(&self, initial_state: SlotState) -> usize {
        let mut slots = self.slots.borrow_mut();
        let idx = slots.len();
        slots.push(SlotModel {
            state: initial_state,
            expected_max_wakes: 0,
        });
        idx
    }

    /// Transitions an existing slot to [`SlotState::Ready`].
    ///
    /// `current_wake_count` is the SUT-observed wake count at the
    /// moment of transition. The model freezes
    /// [`SlotModel::expected_max_wakes`] to this value so that any
    /// subsequent wake (which would be a stale wake on a consumed
    /// future) pushes `wake_count` above the frozen budget and is
    /// caught by the wake-budget invariant.
    pub(super) fn mark_ready(&self, idx: usize, current_wake_count: u32) {
        let mut slots = self.slots.borrow_mut();
        slots[idx].state = SlotState::Ready;
        slots[idx].expected_max_wakes = current_wake_count;
    }

    /// Transitions an existing slot to [`SlotState::Dropped`].
    ///
    /// `current_wake_count` is the SUT-observed wake count at the
    /// moment of transition. Frozen for the same reason as
    /// [`Self::mark_ready`].
    pub(super) fn mark_dropped(&self, idx: usize, current_wake_count: u32) {
        let mut slots = self.slots.borrow_mut();
        slots[idx].state = SlotState::Dropped;
        slots[idx].expected_max_wakes = current_wake_count;
    }
}
