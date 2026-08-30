//! Pseudoconsole PAL: create, resize, close, and byte handles.

use std::fmt;

use crate::pal::error::PalError;
use crate::pal::ids::PtyId;

/// Console size applied to the app pseudoconsole.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct WindowSize {
    /// Width in columns.
    pub cols: u16,
    /// Height in rows.
    pub rows: u16,
}

/// Create and relay a Windows pseudoconsole.
///
/// Whether a child *sees* a console is an integration concern.
/// Ref: docs/implementation.md, PAL slicing and "Pseudoconsole".
#[cfg_attr(test, mockall::automock)]
pub(crate) trait Pseudoconsole: Send + Sync + fmt::Debug + 'static {
    /// Create a pseudoconsole with the given initial size.
    fn create(&self, size: WindowSize) -> Result<PtyId, PalError>;

    /// Apply a new size to the app console.
    fn resize(&self, pty: PtyId, size: WindowSize) -> Result<(), PalError>;

    /// Write bytes to the app's console input. Must not send EOF on client drop.
    fn write_input(&self, pty: PtyId, data: &[u8]) -> Result<(), PalError>;

    /// Read bytes from the app's console output. Blocks until some data arrives.
    fn read_output(&self, pty: PtyId) -> Result<Vec<u8>, PalError>;

    /// End the app console, leaving its remaining output readable.
    ///
    /// Reads keep delivering what the app already wrote and then report the end
    /// of the stream, so a reader can finish the output before the session
    /// reports the exit. The pseudoconsole is released by `close`.
    fn finish(&self, pty: PtyId);

    /// Release the pseudoconsole, abandoning output nobody has read yet.
    fn close(&self, pty: PtyId);
}
