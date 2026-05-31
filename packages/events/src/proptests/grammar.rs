//! Operation grammar and proptest strategies.
//!
//! The grammar defines the alphabet of random actions the harness may
//! generate. Indices carried in `Poll(usize)` / `Drop(usize)` operations
//! are reduced modulo the live-slot count at runtime so they always target
//! some live slot when one exists.

use proptest::collection::vec;
use proptest::prelude::*;

/// Maximum number of slots a sequence may produce. Bounded to keep
/// operation indices small enough that proptest can shrink failures into
/// short reproductions.
pub(super) const MAX_SLOTS: usize = 8;

/// Maximum number of operations per sequence. Each operation touches the
/// system at most a few times; this keeps a single test case fast while
/// still exploring nontrivial interleavings.
pub(super) const OPS_PER_SEQUENCE: usize = 24;

/// What a reentrant waker does when invoked.
///
/// The action runs synchronously inside the production code's wake
/// callback, while the outer `event.set()` is mid-drain. These are the
/// scenarios where reentrancy bugs hide.
#[derive(Clone, Copy, Debug)]
pub(super) enum WakerAction {
    /// Pure tracking waker. No side effects.
    None,
    /// Calls `event.set()` reentrantly. No-op while already set.
    Set,
    /// Calls `event.reset()` reentrantly. Closes the gate mid-drain.
    Reset,
    /// Drops the first other live future. Exercises the sibling-mutation
    /// reentrancy path from PR #141.
    DropFirstOther,
    /// Drops the last other live future. Mirrors `DropFirstOther` at the
    /// opposite end of the awaiter list.
    DropLastOther,
    /// Calls `event.reset()` then registers a fresh waiter with a no-op
    /// waker. The fresh waiter belongs to the new generation and must
    /// remain `Pending` until the next effective `set()`.
    ResetThenRegister,
}

/// A single operation in a generated sequence.
#[derive(Clone, Copy, Debug)]
pub(super) enum Op {
    Set,
    Reset,
    TryWait,
    Register(WakerAction),
    Poll(usize),
    Drop(usize),
}

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

pub(super) fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        2 => Just(Op::Set),
        2 => Just(Op::Reset),
        1 => Just(Op::TryWait),
        4 => waker_action_strategy().prop_map(Op::Register),
        3 => (0_usize..MAX_SLOTS).prop_map(Op::Poll),
        2 => (0_usize..MAX_SLOTS).prop_map(Op::Drop),
    ]
}

pub(super) fn ops_strategy() -> impl Strategy<Value = Vec<Op>> {
    vec(op_strategy(), 1..=OPS_PER_SEQUENCE)
}
