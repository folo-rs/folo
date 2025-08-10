//! Event state machine logic, inspired by the `oneshot` crate.
//!
//! The following states exist:
//!
//! 0 - unbound - this event has not been bound to a sender or receiver;
//!               only used with stand-alone events (pooled events start as bound).
//! 1 - bound - both sender and receiver have been created but have not done anything yet.
//! 2 - set - the sender has set the value of the event but the receiver has not picked it up yet.
//! 3 - awaiting - the receiver is waiting for the sender to set the value.
//! 4 - signaling - the sender is in the process of delivering a wake signal to the awaiter;
//!                 this state is a mutex of sorts, to stop a receiver from updating event state;
//!                 we transition into the "set" state from this state, at which point the receiver
//!                 is welcome to receive the payload.
//! 5 - disconnected - the sender was dropped without setting the event.
//! 6 - consumed - in debug builds, we set this state after both sender and receiver are done;
//!                we never expect this state to be seen but have it here just in case.
//!
//! A key optimization is that the crucial transitions of the sender are a simple `+= 1` operation:
//!
//! * If nobody is listening, we get `bound + 1 = set`
//! * If a receiver is listening, we get `awaiting + 1 = signaling`
//!
//! All other states require the sender to already be dropped, so cannot be increment-transitioned.

pub(crate) const EVENT_UNBOUND: u8 = 0;
pub(crate) const EVENT_BOUND: u8 = 1;
pub(crate) const EVENT_SET: u8 = 2;
pub(crate) const EVENT_AWAITING: u8 = 3;
pub(crate) const EVENT_SIGNALING: u8 = 4;
pub(crate) const EVENT_DISCONNECTED: u8 = 5;
#[cfg(debug_assertions)]
pub(crate) const EVENT_CONSUMED: u8 = 6;
