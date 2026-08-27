//! Typed CLI input consumed by the library entry point.

use std::path::PathBuf;

use crate::session_id::SessionId;

/// Parsed command the binary should execute.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Command {
    /// Start a new session and attach.
    Run {
        /// Command argv after `--`.
        command: Vec<String>,
    },
    /// Attach to an existing live session.
    Resume {
        /// Explicit session id; `None` uses auto-detect.
        id: Option<SessionId>,
    },
    /// Print live sessions.
    List,
    /// Abruptly terminate the recorded supervisor for this id.
    Kill {
        /// Session id to kill. Required by the CLI.
        id: SessionId,
    },
    /// Hidden supervisor role spawned by `run`.
    Supervisor {
        /// One-shot startup pipe name created by the client.
        startup_pipe: String,
        /// Canonical launch directory.
        launch_directory: PathBuf,
        /// Command argv to execute.
        command: Vec<String>,
    },
}

/// Fully parsed invocation.
///
/// Built by [`crate::Cli::into_input`] and consumed by [`crate::run`].
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_structs,
    reason = "handoff struct read directly by the in-crate binary"
)]
pub struct RunInput {
    /// Whether auto-detect should explain its decisions on stderr.
    pub verbose: bool,
    /// Optional store-root override so tests never touch `LocalAppData`.
    pub store_root: Option<PathBuf>,
    /// Subcommand to execute.
    pub command: Command,
}

/// Result of a successful `dure` invocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "handoff enum matched directly by the in-crate binary"
)]
pub enum Outcome {
    /// Command finished without an app exit status to forward.
    Success,
    /// The attached app exited; the process should exit with this status.
    AppExit(i32),
}
