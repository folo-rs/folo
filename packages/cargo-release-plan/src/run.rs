use ohno::AppError;

use crate::apply::run_apply;
use crate::check::run_check;
use crate::report::run_report;
use crate::verbose::Verbose;
use crate::{RunInput, RunOutcome};

/// Core entry point of the tool, extracted for direct testability.
///
/// # Errors
///
/// Returns an application error if Git, Cargo metadata, filesystem access,
/// manifest parsing, or plan validation fails. Unreleased changes are a
/// [`RunOutcome::Check`] with `passed: false`, not an error.
#[doc(hidden)]
pub fn run(input: &RunInput) -> Result<RunOutcome, AppError> {
    match input {
        RunInput::Report {
            out_dir,
            base,
            manifest_path,
            verbose,
        } => {
            let message = run_report(out_dir, base, manifest_path, Verbose::new(*verbose))?;
            Ok(RunOutcome::Report { message })
        }
        RunInput::Check {
            base,
            manifest_path,
            format,
            verify_packaging,
            verbose,
        } => {
            let (passed, message) = run_check(
                base,
                manifest_path,
                *format,
                *verify_packaging,
                Verbose::new(*verbose),
            )?;
            Ok(RunOutcome::Check { passed, message })
        }
        RunInput::Apply {
            plan,
            dry_run,
            manifest_path,
            verbose,
        } => {
            let message = run_apply(plan, *dry_run, manifest_path, Verbose::new(*verbose))?;
            Ok(RunOutcome::Apply { message })
        }
    }
}
