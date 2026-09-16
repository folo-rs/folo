use std::panic::{RefUnwindSafe, UnwindSafe};

use ohno::AppError;

use crate::cli::{Cli, Command};
use crate::errors::{InvalidResponseError, MissingRepositoryError};
use crate::github::WorkflowJob;
use crate::marker::UnexpectedCommentMarker;
use crate::operations::Context;
use crate::workflow::prepare_from_jobs;

/// Runs the native preparation adapters with already-discovered job records.
///
/// This unsupported test entry point accepts a prepare-analysis command with an explicit
/// repository and a JSON array of GitHub job representations. It bypasses only HTTP discovery;
/// it executes the same receipt reconciliation, filesystem adapters and outputs as the CLI.
///
/// # Errors
///
/// Returns an error for invalid preparation arguments, job records or collection artifacts.
// Integration tests need real filesystem coverage without live GitHub access. This private-use
// package is its own implementation crate; no stable library or additional shell is introduced.
#[cfg_attr(test, mutants::skip)]
pub fn prepare_analysis(cli: Cli, jobs_json: &str) -> Result<(), AppError> {
    let context = Context {
        repository: cli.repository().ok_or_else(MissingRepositoryError::new)?,
        instance: cli.instance(),
        verbose: cli.verbose(),
        comment_marker: cli.comment_marker(),
        migration: cli.migration_options()?,
    };
    if context.comment_marker.is_some() {
        return Err(UnexpectedCommentMarker::new().into());
    }
    let Command::PrepareAnalysis(args) = cli.into_command() else {
        return Err(NotPreparationCommand::new().into());
    };
    let jobs: Vec<WorkflowJob> = serde_json::from_str(jobs_json).map_err(|error| {
        InvalidResponseError::caused_by("reading injected workflow jobs", error)
    })?;
    prepare_from_jobs(&context, &args, &jobs)
}

/// Native preparation fixtures require preparation arguments, not a lifecycle command.
#[ohno::error]
#[display("The preparation test adapter requires a prepare-analysis command")]
struct NotPreparationCommand;

impl UnwindSafe for NotPreparationCommand {}
impl RefUnwindSafe for NotPreparationCommand {}
