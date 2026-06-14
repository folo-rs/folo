//! The `analyze` command: examine stored history for notable patterns.
//! Stubbed in Phase 0.

use crate::{AnalyzeOptions, RunError, RunOutcome};

#[expect(
    clippy::unnecessary_wraps,
    reason = "stub returns Ok unconditionally; later iterations return errors"
)]
pub(crate) fn execute(_options: &AnalyzeOptions) -> Result<RunOutcome, RunError> {
    Ok(RunOutcome::NotImplemented { command: "analyze" })
}
