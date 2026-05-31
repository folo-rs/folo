//! Property-based test suite for manual-reset events.
//!
//! This module implements stateful, model-based property testing for the
//! `events` crate's manual-reset event primitives
//! ([`LocalManualResetEvent`](crate::LocalManualResetEvent) and
//! [`ManualResetEvent`](crate::ManualResetEvent)) using the
//! [`proptest`](https://docs.rs/proptest) crate.
//!
//! # How it works
//!
//! The suite drives random operation sequences against two parallel
//! structures:
//!
//! * The **System Under Test (SUT)** — the real production event and its
//!   real wait futures, observed through a [`WakerTracker`] per registered
//!   future.
//! * A **reference [`Model`]** — a parallel data structure that tracks the
//!   *expected* state: whether the event is set, what each slot's lifecycle
//!   phase is, and how many wakes each slot's waker is allowed to have
//!   received.
//!
//! After every operation, [`Harness`] asserts a small set of named
//! invariants that relate the SUT and the model. A divergence indicates a
//! bug in the SUT.
//!
//! # Invariants
//!
//! The event primitive must uphold four invariants, phrased in terms of
//! externally observable behavior. Three have dedicated checks invoked
//! by [`Harness::check_invariants`]; the fourth is implicit (any panic
//! surfaces as a test failure).
//!
//! 1. **No lost notifications.** Every future that is `Pending` at the
//!    instant an effective `set()` call begins must observe `Ready` on its
//!    next poll, even if a reentrant `reset()` runs inside the drain loop.
//!    Checked by `Harness::check_invariant_no_lost_notifications`.
//! 2. **Wake budget.** Each future's waker is invoked at most once per
//!    effective `set()` while the future is `Pending`. After consumption
//!    (`Ready`) or drop, the waker is never invoked again. Checked by
//!    `Harness::check_invariant_wake_budget`.
//! 3. **No stale registration.** A future that returned `Ready` or was
//!    dropped must not appear in the awaiter set; once a slot leaves
//!    `Pending`, its waker must never fire again. Checked by
//!    `Harness::check_invariant_no_stale_registration`. The model
//!    freezes a slot's budget at the moment it transitions to a
//!    terminal state, so any later wake immediately exceeds the
//!    frozen budget and is surfaced by this invariant (and also by
//!    invariant #2 with a less specific message).
//! 4. **No panics.** *Implicit.* Random operation sequences must not
//!    panic. Any panic — in the SUT, the harness, or an invariant
//!    check — surfaces as a proptest failure without a dedicated
//!    invariant function.
//!
//! # Multithreading
//!
//! `proptest` runs operation sequences on a single thread. This suite
//! explores **reentrancy** (re-entering `set`/`reset`/`drop` synchronously
//! from inside a waker callback) and lifecycle ordering, not cross-thread
//! races. True multithreaded race coverage for `ManualResetEvent` lives in
//! the dedicated tests in [`crate::manual`].
//!
//! See GitHub issue #149 for the original pilot motivation.
//!
//! # File map
//!
//! * [`grammar`] — the [`Op`] enum (operation grammar) and proptest
//!   strategies.
//! * [`manual_event`] — the [`ManualEvent`] trait abstracting both
//!   primitive variants.
//! * [`model`] — [`Model`] and [`SlotModel`]: the reference model and its
//!   transition methods.
//! * [`waker_tracker`] — [`WakerTracker`] (live observation) and the raw
//!   waker plumbing.
//! * [`harness`] — [`Harness`] and [`run_sequence`]: the test driver and
//!   invariant checks.
//! * [`local_manual`], [`manual`] — entry points wiring [`Harness`] to
//!   each concrete event type, plus named regression tests.
//!
//! [`Op`]: crate::proptests::grammar::Op
//! [`ManualEvent`]: crate::proptests::manual_event::ManualEvent
//! [`Model`]: crate::proptests::model::Model
//! [`SlotModel`]: crate::proptests::model::SlotModel
//! [`WakerTracker`]: crate::proptests::waker_tracker::WakerTracker
//! [`Harness`]: crate::proptests::harness::Harness
//! [`run_sequence`]: crate::proptests::harness::run_sequence

#![cfg_attr(coverage_nightly, coverage(off))]

mod grammar;
mod harness;
mod local_manual;
mod manual;
mod manual_event;
mod model;
mod waker_tracker;
