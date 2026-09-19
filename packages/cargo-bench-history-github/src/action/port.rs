use std::ffi::OsString;
use std::future::Future;
use std::path::{Path, PathBuf};

use ohno::AppError;

use crate::cli::Command;
use crate::model::Instance;
use crate::operations::Context;

/// Native operations needed by action orchestration, independently replaceable in unit tests.
pub(crate) trait Host {
    fn current_dir(&self) -> Result<PathBuf, AppError>;
    fn environment(&self, name: &str) -> Result<Option<String>, AppError>;
    fn read(&self, path: &Path) -> Result<Vec<u8>, AppError>;
    fn directory(&self, path: &Path) -> Result<PathBuf, AppError>;
    fn output_file(&self, path: &Path) -> Result<PathBuf, AppError>;
    fn outside_checkout(&self, path: &Path, checkout: &Path) -> Result<(), AppError>;
    fn scratch(&self, root: &Path) -> Result<PathBuf, AppError>;
    fn key_files(&self, root: &Path) -> Result<Vec<Vec<u8>>, AppError>;
    fn append_outputs(&self, path: &Path, outputs: &str) -> Result<(), AppError>;
    fn note(&self, message: &str);
    fn instance(
        &self,
        cwd: &Path,
        config: Option<&Path>,
    ) -> impl Future<Output = Result<Instance, AppError>>;
    fn process(&self, process: &Process) -> impl Future<Output = Result<String, AppError>>;
}

/// GitHub construction is deferred until a validated publication actually executes.
pub(crate) trait Publisher {
    fn publish(
        &self,
        command: Command,
        context: &Context,
    ) -> impl Future<Output = Result<(), AppError>>;
}

/// A shell-free process invocation with an explicit output policy and measured checkout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Process {
    pub(crate) program: OsString,
    pub(crate) args: Vec<OsString>,
    pub(crate) cwd: PathBuf,
    pub(crate) output: Output,
}

/// Benchmark logs stream; only dedicated Git and machine-key responses are buffered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Output {
    Inherit,
    Capture,
}
