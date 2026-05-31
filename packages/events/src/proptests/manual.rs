//! Property-based test entry point for [`ManualResetEvent`].
//!
//! See the [`proptests`](crate::proptests) module overview for the
//! shared model and invariants. This file only wires the harness up to
//! the thread-safe primitive and contains hand-written regression
//! tests for known-tricky reentrancy scenarios.
//!
//! These tests cover **single-threaded reentrancy**, not concurrent
//! ordering. Multithreaded race coverage for `ManualResetEvent` lives
//! in the dedicated tests in [`crate::manual`] alongside the
//! Miri-driven test hooks; this suite complements but does not replace
//! those.

use proptest::prelude::*;

use crate::ManualResetEvent;
use crate::proptests::grammar::ops_strategy;
use crate::proptests::harness::run_sequence;

proptest! {
    #![proptest_config(ProptestConfig {
        // Keep CI time bounded. The pilot can be scaled up via the
        // `PROPTEST_CASES` environment variable for deeper bug hunts.
        cases: 256,
        // Disable filesystem regression persistence: we do not want
        // every developer to accumulate `.proptest-regressions` files
        // in their checkout. Shrunk reproductions are still printed on
        // failure, which is the important signal.
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// Combined feasibility test: random operation sequences must
    /// satisfy all four invariants (no lost notifications, wake
    /// budget, no stale registration, no panics). Asserts in the
    /// harness implement invariants 1–3; invariant 4 is implicit in
    /// the absence of panics or aborts.
    #[test]
    // Proptest sequences run hundreds of cases per invocation; each
    // case allocates and drops several futures. Under Miri this takes
    // minutes per test and provides no additional value over the
    // dedicated multithreaded tests in the `manual` module.
    #[cfg_attr(miri, ignore)]
    fn proptest_manual_reset_event(ops in ops_strategy()) {
        run_sequence::<ManualResetEvent>(&ops);
    }
}

#[cfg(test)]
mod regression_tests {
    //! Hand-written sequences that exercise the harness against
    //! known-good scenarios. These run under Miri so the harness's
    //! `unsafe` waker plumbing is validated.

    use crate::ManualResetEvent;
    use crate::proptests::grammar::{Op, WakerAction};
    use crate::proptests::harness::run_sequence;

    #[test]
    fn empty_sequence_is_valid() {
        run_sequence::<ManualResetEvent>(&[]);
    }

    #[test]
    fn set_then_register_returns_ready() {
        run_sequence::<ManualResetEvent>(&[Op::Set, Op::Register(WakerAction::None)]);
    }

    #[test]
    fn register_then_set_then_poll() {
        run_sequence::<ManualResetEvent>(&[Op::Register(WakerAction::None), Op::Set, Op::Poll(0)]);
    }

    #[test]
    fn reentrant_reset_does_not_skip_awaiters() {
        // Mirror of the production test: A's waker resets the event,
        // but B (registered before set) must still be notified.
        run_sequence::<ManualResetEvent>(&[
            Op::Register(WakerAction::Reset),
            Op::Register(WakerAction::None),
            Op::Set,
            Op::Poll(0),
            Op::Poll(0),
        ]);
    }

    #[test]
    fn reentrant_drop_first_other_does_not_skip_others() {
        run_sequence::<ManualResetEvent>(&[
            Op::Register(WakerAction::DropFirstOther),
            Op::Register(WakerAction::None),
            Op::Register(WakerAction::None),
            Op::Set,
            Op::Poll(0),
            Op::Poll(0),
            Op::Poll(0),
        ]);
    }

    #[test]
    fn reentrant_reset_then_register_remains_pending() {
        // The fresh waiter belongs to the new generation; the outer
        // set's drain skips it. After all polls, the fresh future must
        // remain Pending.
        run_sequence::<ManualResetEvent>(&[Op::Register(WakerAction::ResetThenRegister), Op::Set]);
    }
}
