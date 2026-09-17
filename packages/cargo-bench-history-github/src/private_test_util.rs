use std::num::NonZero;
use std::panic::{RefUnwindSafe, UnwindSafe};

use ohno::AppError;
use serde_json::{Value, json};
use tick::Clock;

use crate::cli::{Cli, Command};
use crate::errors::{InvalidResponseError, MissingRepositoryError};
use crate::github::WorkflowJob;
use crate::github::fake::FakeGitHub;
use crate::operations::{Context, dispatch};
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
    };
    let Command::PrepareAnalysis(args) = cli.into_command() else {
        return Err(NotPreparationCommand::new().into());
    };
    let jobs: Vec<WorkflowJob> = serde_json::from_str(jobs_json).map_err(|error| {
        InvalidResponseError::caused_by("reading injected workflow jobs", error)
    })?;
    prepare_from_jobs(&context, &args, &jobs)
}

/// Executes native command adapters against in-memory GitHub state.
///
/// This unsupported integration hook requires explicit repositories and an injected clock.
/// Commands share GitHub state; each resulting snapshot contains issues and the selected PR's
/// comments. Report loading, workflow files and command dispatch execute unchanged.
///
/// # Errors
///
/// Returns an error for unsupported commands, malformed fixtures or command failures.
#[cfg_attr(test, mutants::skip)]
pub async fn run_commands(
    commands: Vec<Cli>,
    clock: Clock,
    pull_request: NonZero<u64>,
    head: &str,
    jobs_json: &str,
) -> Result<String, AppError> {
    let github = FakeGitHub::new();
    github.set_pull_head(pull_request.get(), head.parse()?);
    github.set_jobs(serde_json::from_str(jobs_json).map_err(|error| {
        InvalidResponseError::caused_by("reading injected workflow jobs", error)
    })?);
    let mut snapshots = Vec::new();
    for cli in commands {
        let context = Context {
            repository: cli.repository().ok_or_else(MissingRepositoryError::new)?,
            instance: cli.instance(),
            verbose: cli.verbose(),
        };
        let command = cli.into_command();
        if matches!(
            command,
            Command::WorkflowMatrix(_) | Command::CollectionReceipt(_) | Command::InspectReport(_)
        ) {
            return Err(NotOnlineCommand::new().into());
        }
        dispatch(command, &context, &github, &clock).await?;
        let issues: Vec<_> = github.issues().into_iter().map(|issue| json!({
            "number": issue.number, "title": issue.title, "body": issue.body, "open": issue.open
        })).collect();
        let comments: Vec<_> = github
            .comments_for(pull_request.get())
            .into_iter()
            .map(|comment| json!({"id": comment.id, "body": comment.body}))
            .collect();
        snapshots.push(json!({"issues": issues, "comments": comments}));
    }
    Ok(Value::Array(snapshots).to_string())
}

/// Native preparation fixtures require preparation arguments, not a lifecycle command.
#[ohno::error]
#[display("The preparation test adapter requires a prepare-analysis command")]
struct NotPreparationCommand;

/// This hook covers online dispatch; offline commands run directly through the binary.
#[ohno::error]
#[display("The command test adapter requires an online command")]
struct NotOnlineCommand;

impl UnwindSafe for NotPreparationCommand {}
impl RefUnwindSafe for NotPreparationCommand {}
impl UnwindSafe for NotOnlineCommand {}
impl RefUnwindSafe for NotOnlineCommand {}
