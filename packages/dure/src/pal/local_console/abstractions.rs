//! Local console PAL used by the client process.

use crate::pal::error::PalError;
use crate::pal::pseudoconsole::WindowSize;

/// One blocking read from the local console during attach.
#[derive(Clone, Debug, Eq, PartialEq)]
#[cfg_attr(
    all(not(windows), not(test)),
    expect(
        dead_code,
        reason = "variants are constructed by the Windows console PAL and by tests"
    )
)]
pub(crate) enum ConsoleInput {
    /// VT or key bytes to forward as [`crate::protocol::Message::Input`].
    Bytes(Vec<u8>),
    /// Window size changed; forward as [`crate::protocol::Message::Resize`].
    Resize(WindowSize),
}

/// Detect a console, switch it to a raw relay, and exchange bytes.
///
/// Ref: docs/implementation.md, PAL slicing.
#[cfg_attr(test, mockall::automock)]
pub(crate) trait LocalConsole: Send + Sync + std::fmt::Debug + 'static {
    /// Whether this process is attached to a console.
    fn has_console(&self) -> bool;

    /// Whether stdin can be used for an interactive id prompt.
    fn stdin_is_terminal(&self) -> bool;

    /// Disable the client's Ctrl+C handler so the key is forwarded.
    fn disable_ctrl_c_handler(&self) -> Result<(), PalError>;

    /// Put the console into a raw VT relay.
    fn enter_raw_relay(&self) -> Result<(), PalError>;

    /// Restore console modes saved by [`LocalConsole::enter_raw_relay`].
    fn leave_raw_relay(&self) -> Result<(), PalError>;

    /// Current console size.
    fn window_size(&self) -> Result<WindowSize, PalError>;

    /// Blocking read of console input bytes or a window-size change.
    fn read_input(&self) -> Result<ConsoleInput, PalError>;

    /// Write console output bytes.
    fn write_output(&self, data: &[u8]) -> Result<(), PalError>;

    /// Read one line from stdin for the resume-id prompt.
    fn read_prompt_line(&self) -> Result<String, PalError>;
}
