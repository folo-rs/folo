//! Dispatch: routes a parsed [`Command`](crate::Command) to the handler that
//! executes it.

use std::path::PathBuf;

use jiff::Timestamp;

use crate::commands;
use crate::{Command, RunError, RunOutcome};

/// Test-only overrides for [`run_with_overrides`].
///
/// Production code calls [`run`], which supplies [`Overrides::default`] — every
/// field `None`. With all fields `None` a command resolves its workspace from the
/// process working directory, runs `cargo bench` as the benchmark command with the
/// conventional target root, and anchors the analysis `--since` window to the wall
/// clock. Tests set individual fields to drive the full flow hermetically: a
/// `workspace_dir` lets a command operate on a temporary directory without a global
/// `chdir`, so the test suite is not forced serial.
#[doc(hidden)]
#[derive(Debug, Default)]
#[expect(
    clippy::exhaustive_structs,
    reason = "constructed by the integration and Azure test harnesses by struct literal"
)]
pub struct Overrides {
    /// The workspace directory the command operates on. `None` resolves it from
    /// the process working directory. Relative `--config`/`--repo`/local-storage
    /// paths resolve against this directory.
    pub workspace_dir: Option<PathBuf>,
    /// The cargo target root scanned for engine output (`run`/`backfill`). `None`
    /// resolves the conventional target directory under the workspace.
    pub target_root: Option<PathBuf>,
    /// The benchmark command `run`/`backfill` invoke instead of `cargo bench`.
    pub bench_command: Option<Vec<String>>,
    /// The clock anchor for the analysis default `--since` lookback window.
    pub now: Option<Timestamp>,
}

/// Executes a parsed command.
///
/// Every handler is asynchronous: `run` and `analyze` drive IO over the
/// configured storage, and `install` writes a configuration file.
///
/// # Errors
///
/// Returns a `RunError` if a command fails (for example, a configuration or
/// storage error).
#[doc(hidden)]
pub async fn run(command: &Command) -> Result<RunOutcome, RunError> {
    run_with_overrides(command, Overrides::default()).await
}

/// Executes a parsed command with the given test [`Overrides`].
///
/// This exists so end-to-end tests can drive the full `run`/`backfill` flow
/// against a mock benchmark program instead of `cargo bench`, operate on a
/// temporary workspace without mutating the process environment or current
/// directory, and pin a deterministic `now` for the analysis default `--since`
/// window. Production code calls [`run`], which passes [`Overrides::default`].
///
/// # Errors
///
/// Returns a `RunError` if a command fails (for example, a configuration or
/// storage error).
#[doc(hidden)]
pub async fn run_with_overrides(
    command: &Command,
    overrides: Overrides,
) -> Result<RunOutcome, RunError> {
    let Overrides {
        workspace_dir,
        target_root,
        bench_command,
        now,
    } = overrides;
    let workspace_dir = match workspace_dir {
        Some(dir) => dir,
        None => std::env::current_dir().map_err(RunError::Io)?,
    };
    let workspace_dir = workspace_dir.as_path();
    match command {
        Command::Run(options) => {
            commands::run(options, workspace_dir, target_root, bench_command).await
        }
        Command::Install(options) => commands::install(options, workspace_dir).await,
        Command::Analyze(options) => commands::analyze(options, workspace_dir, now).await,
        Command::List(options) => commands::list(options, workspace_dir, now).await,
        Command::Clean(options) => commands::clean(options, workspace_dir).await,
        Command::Backfill(options) => {
            commands::backfill(options, workspace_dir, bench_command).await
        }
        Command::Bless(options) => commands::bless(options, workspace_dir, now).await,
        Command::Unbless(options) => commands::unbless(options, workspace_dir).await,
    }
}
