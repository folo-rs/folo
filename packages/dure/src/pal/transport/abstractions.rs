//! Client-supervisor transport PAL.

use std::time::Duration;

use crate::pal::error::PalError;
use crate::pal::ids::{ConnId, ListenerId};
use crate::protocol::Message;

/// Byte-stream between one supervisor and its attaching clients.
///
/// Steal is "accept a new connection while an old one still exists."
/// Ref: docs/implementation.md, PAL slicing and "Transport".
#[cfg_attr(test, mockall::automock)]
pub(crate) trait Transport: Send + Sync + std::fmt::Debug + 'static {
    /// Create a first-instance listener for `name`.
    fn listen(&self, name: &str) -> Result<ListenerId, PalError>;

    /// Block until a client connects.
    fn accept(&self, listener: ListenerId) -> Result<ConnId, PalError>;

    /// Connect to `name`, failing with [`crate::pal::error::PalErrorKind::Timeout`]
    /// if the wait elapses.
    fn connect(&self, name: &str, timeout: Duration) -> Result<ConnId, PalError>;

    /// Send one framed message.
    fn send(&self, conn: ConnId, message: &Message) -> Result<(), PalError>;

    /// Receive one framed message, blocking until one arrives or the peer drops.
    fn recv(&self, conn: ConnId) -> Result<Message, PalError>;

    /// Drop the connection so the peer unblocks.
    fn disconnect(&self, conn: ConnId);

    /// Stop accepting. Unblocks a thread waiting in [`Transport::accept`].
    fn close_listener(&self, listener: ListenerId);

    /// Build a per-session pipe name containing `nonce`.
    fn pipe_name(&self, nonce: &str) -> String;
}
