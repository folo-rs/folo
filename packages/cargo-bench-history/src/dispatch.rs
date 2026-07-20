//! Dispatch: routes a parsed [`Command`](crate::Command) to the handler that
//! executes it.

use std::path::PathBuf;

use cbh_analyze::AutoFacets;
use cbh_storage::StorageOverride;
use tick::Clock;

use crate::{Command, RunError, RunOutcome, commands};

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
pub struct Overrides {
    /// The workspace directory the command operates on. `None` resolves it from
    /// the process working directory. Relative `--config`/`--repo`/local-storage
    /// paths resolve against this directory.
    pub workspace_dir: Option<PathBuf>,
    /// The cargo target root scanned for engine output (`collect`/`backfill`). `None`
    /// resolves the conventional target directory under the workspace.
    pub target_root: Option<PathBuf>,
    /// The benchmark command `collect`/`backfill` invoke instead of `cargo bench`.
    pub bench_command: Option<Vec<String>>,
    /// The clock the analysis family reads "now" from — the default `--since`
    /// lookback anchor and each blessing's issue time. `None` uses the runtime
    /// wall clock; tests inject a frozen `Clock` for deterministic windows.
    pub clock: Option<Clock>,
    /// A pre-built storage backend for `collect`/`analyze` to use instead of the one
    /// resolved from configuration. End-to-end tests use this to drive commands
    /// against an Azurite backend behind a locally-faked Entra token and a
    /// certificate-trusting transport, which no configuration could produce.
    pub storage_override: Option<StorageOverride>,
    /// The auto-detected discriminant facets (host target triple + machine key) the
    /// query commands default to when a facet is omitted. `None` probes the host;
    /// integration tests inject fixed values so the suite is independent of the host
    /// it runs on.
    pub auto_facets: Option<AutoFacets>,
}

/// Executes a parsed command.
///
/// Every handler is asynchronous: `collect` and `analyze` drive IO over the
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
/// This exists so end-to-end tests can drive the full `collect`/`backfill` flow
/// against a mock benchmark program instead of `cargo bench`, operate on a
/// temporary workspace without mutating the process environment or current
/// directory, and inject a deterministic [`tick::Clock`] for the analysis default
/// `--since` window. Production code calls [`run`], which passes
/// [`Overrides::default`].
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
        clock,
        storage_override,
        auto_facets,
    } = overrides;
    let workspace_dir = match workspace_dir {
        Some(dir) => dir,
        None => std::env::current_dir().map_err(RunError::Io)?,
    };
    let workspace_dir = workspace_dir.as_path();
    let storage_override = storage_override.map(StorageOverride::into_facade);
    match command {
        Command::Collect(options) => {
            commands::collect(
                options,
                workspace_dir,
                target_root,
                bench_command,
                storage_override,
            )
            .await
        }
        Command::Install(options) => commands::install(options, workspace_dir).await,
        Command::Import(options) => {
            commands::import(options, workspace_dir, storage_override).await
        }
        Command::Analyze(options) => {
            commands::analyze(options, workspace_dir, clock, storage_override, auto_facets).await
        }
        Command::List(options) => {
            commands::list(options, workspace_dir, clock, storage_override, auto_facets).await
        }
        Command::Examine(options) => {
            commands::examine(options, workspace_dir, clock, storage_override, auto_facets).await
        }
        Command::Prune(options) => {
            commands::prune(options, workspace_dir, clock, storage_override, auto_facets).await
        }
        Command::Backfill(options) => {
            commands::backfill(options, workspace_dir, bench_command).await
        }
        Command::Bless(options) => {
            commands::bless(options, workspace_dir, clock, auto_facets).await
        }
        Command::Unbless(options) => commands::unbless(options, workspace_dir, auto_facets).await,
        Command::MachineKey(options) => commands::machine_key(options).await,
    }
}
