use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use crp_native::command::install_cancellation_handler;
use ohno::AppError;

use crate::PublicationOutput;
use crate::legacy::binary_cli::Cli;
use crate::legacy::model::{Batch, Plan};
use crate::legacy::plan::plan;
use crate::publication::binaries::model::InvalidPlan;
use crate::publication::binaries::{BinaryPublisher, Github, Outcome, execute_items};

// Private item-array artifact used by release-binaries consumers and executable smoke tests.
// Ref: packages/release-binaries/docs/implementation.md, "Bootstrap binary command adapter".
const OUTCOMES_FILE: &str = "outcomes.json";

/// Preserves the bootstrap executable's JSON, receipt and summary protocols.
///
/// The shell supplies its current directory, optional summary destination and diagnostics.
/// A JSON string is returned only for `plan`; `run` records item outcomes and diagnostics.
pub fn run_binaries(
    arguments: impl IntoIterator<Item = OsString>,
    cwd: &Path,
    summary_path: Option<&Path>,
    diagnostics: &PublicationOutput,
) -> Result<Option<String>, AppError> {
    let cli = Cli::parse(arguments)?;
    install_cancellation_handler()?;
    match cli {
        Cli::Plan { input, repository } => {
            let input: Plan = serde_json::from_slice(&fs::read(cwd.join(input))?)?;
            let github = Github::new(repository, diagnostics.clone());
            let batches = plan(input, |binary| github.assets(binary, cwd), diagnostics)?;
            Ok(Some(serde_json::to_string(&batches)?))
        }
        Cli::Run {
            input,
            repository,
            controller,
            output,
            no_upload,
        } => {
            let batch: Batch = serde_json::from_slice(&fs::read(cwd.join(input))?)?;
            batch.validate()?;
            let github = Github::new(repository, diagnostics.clone());
            if !no_upload {
                // Planning and execution are separate jobs. Rebind every tag before current
                // asset observations can authorize either a skip or delivery from its frozen source.
                github.verify_sources(&batch.binaries, cwd)?;
            }
            let output = cwd.join(output);
            let mut executor = BinaryPublisher::new(
                cwd.join(controller),
                output.clone(),
                batch.triple.clone(),
                github,
            )?;
            let outcomes = execute_items(
                &batch.triple,
                &batch.binaries,
                no_upload,
                &mut executor,
                diagnostics,
            )?;
            fs::write(
                output.join(OUTCOMES_FILE),
                serde_json::to_vec_pretty(&outcomes)?,
            )?;
            let summary = summary(&batch.triple, &outcomes)?;
            for outcome in &outcomes {
                if let Some(error) = &outcome.cleanup_error {
                    diagnostics.line(format_args!(
                        "{}: source cleanup error: {error}",
                        outcome.binary.tag
                    ));
                }
            }
            diagnostics.line(format_args!("{summary}"));
            if let Some(path) = summary_path {
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(cwd.join(path))?
                    .write_all(summary.as_bytes())?;
            }
            if !complete(&outcomes) {
                return Err(InvalidPlan::new(format!(
                    "Release batch contains failed items; see {OUTCOMES_FILE} and the job summary"
                ))
                .into());
            }
            Ok(None)
        }
    }
}

fn summary(target: &str, outcomes: &[Outcome]) -> Result<String, AppError> {
    let mut summary = format!(
        "## Release binaries: {target}\n\n| Package | Version | Source | Outcome | Stage |\n|---|---|---|---|---|\n"
    );
    for outcome in outcomes {
        writeln!(
            summary,
            "| {} | {} | {} | {} | {} |",
            outcome.binary.name,
            outcome.binary.version,
            outcome.binary.source_sha,
            outcome.status,
            outcome.stage,
        )?;
    }
    Ok(summary)
}

fn complete(outcomes: &[Outcome]) -> bool {
    outcomes.iter().all(|outcome| {
        !matches!(outcome.status.as_str(), "failed" | "unattempted")
            && outcome.cleanup_error.is_none()
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::slice;

    use super::*;
    use crate::publication::binaries::model::tests::binary;

    #[test]
    fn item_failures_and_cleanup_failures_keep_the_legacy_command_failing() {
        let mut item = Outcome {
            binary: binary("tool"),
            status: "published".to_owned(),
            stage: "upload".to_owned(),
            diagnostic: None,
            cleanup_error: None,
        };
        for status in [
            "published",
            "skipped-complete",
            "staged-only",
            "failed",
            "unattempted",
        ] {
            item.status = status.to_owned();
            assert_eq!(
                complete(slice::from_ref(&item)),
                !matches!(status, "failed" | "unattempted"),
            );
        }
        item.status = "published".to_owned();
        item.cleanup_error = Some("cleanup canary".to_owned());
        assert!(!complete(slice::from_ref(&item)));
        let summary = summary("native", slice::from_ref(&item)).unwrap();
        assert!(summary.contains("| tool | 1.2.3 |"));
        assert!(summary.contains(&item.binary.source_sha));
        assert!(summary.contains("| published | upload |"));
    }
}
