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
//!                 Only the thread-safe `Event` variant ever observes this state; the
//!                 single-threaded `LocalEvent` variant transitions directly from `awaiting`
//!                 to `set` because no concurrent receiver can witness the intermediate value.
//! 4 - disconnected - one of the endpoints has disconnected before completing the send/receive.
//!
//! A key optimization in the thread-safe `Event` variant is that the "send" transition is a
//! simple `+= 1` operation, executed via a single atomic `fetch_add`:
//!
//! * If nobody is listening, we get `bound + 1 = set`
//! * If a receiver is listening, we get `awaiting + 1 = signaling`
//!
//! This trick is specific to the thread-safe variant because it compresses the read-and-write
//! into one atomic instruction. The single-threaded `LocalEvent` variant cannot benefit from
//! the encoding (a non-atomic `Cell` `+= 1` lowers to a separate load + add + store), so it
//! transitions directly to the target state instead.
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
