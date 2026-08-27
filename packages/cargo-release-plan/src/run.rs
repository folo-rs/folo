use std::path::PathBuf;

use ohno::AppError;

use crate::apply::run_apply;
use crate::check::{CheckFormat, run_check};
use crate::report::run_report;
use crate::verbose::Verbose;

/// Input parameters for [`run`].
#[doc(hidden)]
#[derive(Debug)]
#[expect(
    clippy::exhaustive_enums,
    reason = "Hidden enum for internal/test use only"
)]
pub enum RunInput {
    /// `report` — write `report.json` and per-package diffs.
    Report {
        /// Directory that receives `report.json` and `diffs/`.
        out_dir: PathBuf,
        /// Base revision whose first-parent line supplies anchors.
        base: String,
        /// Workspace manifest to classify. Used verbatim.
        manifest_path: PathBuf,
        /// When set, print explanatory decision notes to stderr.
        verbose: bool,
    },
    /// `check` — fail on unreleased changes or an inconsistent group.
    Check {
        /// Base revision whose first-parent line supplies anchors.
        base: String,
        /// Workspace manifest to classify. Used verbatim.
        manifest_path: PathBuf,
        /// How to render diagnostics.
        format: CheckFormat,
        /// When set, warn on divergence from `cargo package --list` without failing.
        verify_packaging: bool,
        /// When set, print explanatory decision notes to stderr.
        verbose: bool,
    },
    /// `apply` — rewrite manifests according to an approved plan.
    Apply {
        /// Path to the plan JSON file.
        plan: PathBuf,
        /// When set, compute edits without writing files or refreshing the lockfile.
        dry_run: bool,
        /// Workspace manifest to edit. Used verbatim.
        manifest_path: PathBuf,
        /// When set, print explanatory decision notes to stderr.
        verbose: bool,
    },
}

/// The successful outcome of a run.
#[doc(hidden)]
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "Hidden enum for internal/test use only"
)]
pub enum RunOutcome {
    /// `report` finished and wrote its artifacts.
    Report {
        /// Human-readable summary for stdout. Empty when there is nothing to say.
        message: String,
    },
    /// `check` finished. `passed` is the process-level verdict.
    Check {
        /// Whether every publishable package and version group passed.
        passed: bool,
        /// Rendered diagnostics or a success summary.
        message: String,
    },
    /// `apply` finished (including `--dry-run`).
    Apply {
        /// Human-readable summary for stdout.
        message: String,
    },
}

/// Core entry point of the tool, extracted for direct testability.
///
/// # Errors
///
/// Returns an application error when the requested operation cannot be
/// completed. Unreleased changes are a [`RunOutcome::Check`] with
/// `passed: false`, not an error.
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(RunInput: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RunOutcome: UnwindSafe, RefUnwindSafe);
}
