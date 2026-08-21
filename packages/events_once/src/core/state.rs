//! Event state machine logic, shared by the thread-safe `Event` and the single-threaded
//! `LocalEvent`. This module is the canonical definition of what each state means; the
//! implementations and their documentation defer to the vocabulary established here instead
//! of restating it. See `docs/implementation.md` for how the state machine fits into the
//! package architecture.
//!
//! Every event starts in the bound state with both endpoints present (it is up to the event
//! lifecycle code to ensure an event is never created without both endpoints).
//!
//! # States
//!
//! | State | Meaning |
//! | --- | --- |
//! | `bound` | Neither endpoint has acted yet. |
//! | `set` | The sender has published a payload that the receiver has not yet extracted. |
//! | `awaiting` | The receiver has registered an awaiter and is waiting for the sender. |
//! | `signaling` | The sender has taken exclusive ownership of the awaiter to deliver a wake signal. |
//! | `disconnected` | An endpoint has ended the exchange without a payload changing hands. |
//!
//! # Completion versus payload availability
//!
//! The `set` and `disconnected` states are terminal: reception can complete immediately and
//! without further cooperation from the other endpoint. Completion is what readiness means -
//! a terminal event yields either a payload (`set`) or a disconnection (`disconnected`), so
//! readiness alone never promises that a payload exists.
//!
//! The `signaling` state is not terminal even though a payload may already be present in
//! storage: the sender still owns the event fields and has not published the terminal state,
//! so the payload is not yet extractable. Synchronous inspection therefore treats `signaling`
//! the same as `bound` and `awaiting`.
//!
//! # Field initialization
//!
//! The state is the only record of which event fields hold an initialized value:
//!
//! * `value` is initialized in the `set` state.
//! * `awaiter` is initialized in the `awaiting` state, and in the `signaling` state until the
//!   sender moves the waker out.
//!
//! An endpoint that extracts a payload or an awaiter leaves the field uninitialized while the
//! state still advertises it. Such a window is closed before any other party can observe it:
//! the extracting endpoint either publishes the next state or is the last endpoint and releases
//! the event without any further state-driven cleanup.
//!
//! # Cleanup ownership
//!
//! The states also assign which endpoint releases the event storage:
//!
//! * If the receiver disconnects first, it publishes `disconnected` and the sender releases
//!   the event.
//! * Otherwise the receiver releases the event, having reached either a payload or a
//!   sender-side disconnect.
//!
//! Exactly one endpoint receives this assignment for any given event, and it receives the
//! assignment as the result of the state transition it performed. That endpoint is the only
//! party permitted to release the event, and no endpoint may access the event after the
//! transition that transferred cleanup ownership to the other endpoint.
//!
//! # Callback publication order
//!
//! Completing or cancelling an event runs user-supplied waker callbacks that may reenter the
//! event, including polling the receiver and dropping the last endpoint. The terminal state is
//! therefore always published before such a callback runs, so a reentrant caller observes a
//! terminal state rather than the transient `signaling` state. See `docs/callback-safety.md`.
//!
//! # The signaling state
//!
//! Only the thread-safe `Event` variant ever observes `signaling`. It acts as a mutex of sorts
//! that stops the receiver from updating event state while the sender takes the awaiter, and it
//! is left for `set` or `disconnected` within a few instructions. The single-threaded
//! `LocalEvent` variant transitions directly to the terminal state because no concurrent
//! receiver can witness an intermediate state.
//!
//! The numeric encoding exists to serve the thread-safe variant, whose "send" transition is a
//! single atomic `fetch_add` of 1:
//!
//! * If nobody is listening, we get `bound + 1 = set`
//! * If a receiver is listening, we get `awaiting + 1 = signaling`
//!
//! This compresses a read-and-write into one atomic instruction. The single-threaded variant
//! cannot benefit from the encoding (a non-atomic `Cell` `+= 1` lowers to a separate load + add
//! + store), so it transitions directly to the target state instead.

pub(crate) const EVENT_BOUND: u8 = 0;
pub(crate) const EVENT_SET: u8 = 1;
pub(crate) const EVENT_AWAITING: u8 = 2;
pub(crate) const EVENT_SIGNALING: u8 = 3;
pub(crate) const EVENT_DISCONNECTED: u8 = 4;
