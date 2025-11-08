//! Event state machine logic. Everything starts in the bound state, in which
//! we also assume that both the sender and receiver exist (it is up to the event
//! lifecycle code to ensure an event is never created without both endpoints).
//!
//! The following states exist:
//!
//! 0 - bound - initial state; neither sender nor receiver has done anything yet.
//! 1 - set - the sender has set the value of the event but the receiver has not picked it up yet.
//! 2 - awaiting - the receiver is waiting for the sender to set the value.
//! 3 - signaling - the sender is in the process of delivering a wake signal to the awaiter;
//!                 this state is a mutex of sorts, to stop a receiver from updating event state;
//!                 we transition into the "set" state from this state, at which point the receiver
//!                 is welcome to receive the payload.
//! 4 - disconnected - one of the endpoints has disconnected before completing the send/receive.
//! 
//! A key optimization is that the "send" transition is a simple `+= 1` operation:
//!
//! * If nobody is listening, we get `bound + 1 = set`
//! * If a receiver is listening, we get `awaiting + 1 = signaling`
//!
//! These states are also used to coordinate which of the endpoints drops the event itself:
//! * If the receiver disconnects first, it will set the `disconnected` state and the sender
//!   will be responsible for cleaning up the event.
//! * Otherwise, the receiver is responsible for cleaning up the event (which will end up either
//!   with a value or with a sender-side disconnect).

pub(crate) const EVENT_BOUND: u8 = 0;
pub(crate) const EVENT_SET: u8 = 1;
pub(crate) const EVENT_AWAITING: u8 = 2;
pub(crate) const EVENT_SIGNALING: u8 = 3;
pub(crate) const EVENT_DISCONNECTED: u8 = 4;
