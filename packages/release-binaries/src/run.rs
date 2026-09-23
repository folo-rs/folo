use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::process::ExitCode;

use ohno::AppError;

use crate::batch::execute;
use crate::cli::Cli;
use crate::command::install_cancellation_handler;
use crate::model::{Batch, InvalidPlan, Plan};
use crate::native::{Github, Native};
use crate::plan::plan;

/// Runs the private workflow controller and reports a failing exit for incomplete batches.
// This is the executable's environment/filesystem shell; pure decisions live in their modules.
#[cfg_attr(test, mutants::skip)]
#[must_use]
pub fn run() -> ExitCode {
    match dispatch() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

// File IO and executable wiring are covered through the binary's integration tests.
#[cfg_attr(test, mutants::skip)]
fn dispatch() -> Result<(), AppError> {
    install_cancellation_handler()?;
    match Cli::parse(env::args_os().skip(1))? {
        Cli::Plan { input, repository } => {
            let input: Plan = serde_json::from_slice(&fs::read(input)?)?;
            let github = Github::new(repository);
            let cwd = env::current_dir()?;
            let batches = plan(input, |binary| github.assets(binary, &cwd))?;
            println!("{}", serde_json::to_string(&batches)?);
        }
        Cli::Run {
            input,
            repository,
            controller,
            output,
            no_upload,
        } => {
            let batch: Batch = serde_json::from_slice(&fs::read(input)?)?;
            batch.validate()?;
            let mut executor =
                Native::new(controller, output.clone(), batch.triple.clone(), repository)?;
            let outcomes = execute(&batch, no_upload, &mut executor)?;
            fs::write(
                output.join("outcomes.json"),
                serde_json::to_vec_pretty(&outcomes)?,
            )?;
            let mut summary = format!(
                "## Release binaries: {}\n\n| Package | Version | Source | Outcome | Stage |\n|---|---|---|---|---|\n",
                batch.triple
            );
            for outcome in &outcomes {
                use std::fmt::Write;
                writeln!(
                    summary,
                    "| {} | {} | {} | {} | {} |",
                    outcome.binary.name,
                    outcome.binary.version,
                    outcome.binary.source_sha,
                    outcome.status,
                    outcome.stage,
                )?;
                if let Some(error) = &outcome.cleanup_error {
                    eprintln!("{}: source cleanup error: {error}", outcome.binary.tag);
                }
            }
            eprintln!("{summary}");
            if let Some(path) = env::var_os("GITHUB_STEP_SUMMARY") {
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)?
                    .write_all(summary.as_bytes())?;
            }
            if outcomes.iter().any(|outcome| {
                outcome.status == "failed"
                    || outcome.status == "unattempted"
                    || outcome.cleanup_error.is_some()
            }) {
                return Err(InvalidPlan::new(
                    "Release batch contains failed items; see outcomes.json and the job summary"
                        .to_owned(),
                )
                .into());
            }
        }
    }
    Ok(())
}
