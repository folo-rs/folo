//! The session request, independent of its platform resources.

use std::path::PathBuf;

use crate::AppCommand;

/// What the supervisor is being asked to run.
pub(crate) struct SessionSpec {
    /// Canonical launch directory used by resume selection.
    pub(crate) launch_directory: PathBuf,
    /// Executable and arguments the session runs.
    pub(crate) command: AppCommand,
    /// Publication time for the displayed session age.
    pub(crate) started_at_unix_ms: u64,
}
