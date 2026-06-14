//! The `install` command: generate a starter configuration file.
//! Stubbed in Phase 0.

use crate::{InstallOptions, RunError, RunOutcome};

#[expect(
    clippy::unnecessary_wraps,
    reason = "stub returns Ok unconditionally; later iterations return errors"
)]
pub(crate) fn execute(_options: &InstallOptions) -> Result<RunOutcome, RunError> {
    Ok(RunOutcome::NotImplemented { command: "install" })
}
