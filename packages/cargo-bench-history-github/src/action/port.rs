use std::ffi::OsString;
use std::future::Future;
use std::path::{Path, PathBuf};

use ohno::AppError;

use crate::cli::Command;
use crate::model::Instance;
use crate::operations::Context;

/// Native operations needed by action orchestration, independently replaceable in unit tests.
pub(crate) trait Host {
    /// Anchors relative invocation inputs without changing the process working directory.
    fn current_dir(&self) -> Result<PathBuf, AppError>;
    /// Reads one requested context variable; orchestration chooses the noncredential names.
    fn environment(&self, name: &str) -> Result<Option<String>, AppError>;
    /// Supplies local input or report bytes to the shared parsing and validation logic.
    fn read(&self, path: &Path) -> Result<Vec<u8>, AppError>;
    /// Queries package ownership without executing a command or changing process CWD.
    fn package(&self, workspace: &Path, target: &Path) -> Result<Option<String>, AppError>;
    /// Resolves an existing directory for measured-checkout and physical-path checks.
    fn directory(&self, path: &Path) -> Result<PathBuf, AppError>;
    /// Validates an output destination without appending success records yet.
    fn output_file(&self, path: &Path) -> Result<PathBuf, AppError>;
    /// Checks physical separation from the checkout before analysis writes artifacts or cache.
    fn outside_checkout(&self, path: &Path, checkout: &Path) -> Result<(), AppError>;
    /// Creates an invocation-owned report directory that remains available for later upload.
    fn scratch(&self, root: &Path) -> Result<PathBuf, AppError>;
    /// Loads the selected key-file tree; hardware validation and deduplication are separate.
    fn key_files(&self, root: &Path) -> Result<Vec<Vec<u8>>, AppError>;
    /// Appends the caller's completed output block without replacing earlier step records.
    fn append_outputs(&self, path: &Path, outputs: &str) -> Result<(), AppError>;
    /// Emits the orchestration decision context needed to explain a workflow run.
    fn note(&self, message: &str);
    /// Resolves the project namespace using the same configuration/storage identity as the core.
    fn instance(
        &self,
        cwd: &Path,
        config: Option<&Path>,
    ) -> impl Future<Output = Result<Instance, AppError>>;
    /// Executes the planned argument vector with its explicit checkout and stream policy.
    fn process(&self, process: &Process) -> impl Future<Output = Result<String, AppError>>;
}

/// GitHub construction is deferred until a validated publication actually executes.
pub(crate) trait Publisher {
    /// Runs a typed lifecycle command only after root-action input and fork checks.
    fn publish(
        &self,
        command: Command,
        context: &Context,
    ) -> impl Future<Output = Result<(), AppError>>;
}

/// A shell-free invocation with a measured checkout and child-only environment overrides.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Process {
    pub(crate) program: OsString,
    pub(crate) args: Vec<OsString>,
    pub(crate) cwd: PathBuf,
    pub(crate) output: Output,
    /// Empty overrides preserve the full inherited environment.
    pub(crate) env: Vec<(OsString, OsString)>,
}

/// Benchmark logs stream; only dedicated Git and machine-key responses are buffered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Output {
    Inherit,
    Capture,
}
