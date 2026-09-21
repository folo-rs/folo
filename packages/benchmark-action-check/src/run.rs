use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::Parser;
use ohno::AppError;

use crate::check::pairing_needed;

/// Emits the one-line relevance result consumed by repository release instructions.
#[must_use]
pub fn run() -> ExitCode {
    match execute(Args::parse()) {
        Ok(needed) => {
            println!("{needed}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

/// Inputs to the repo-specific relevance check, not another version-planning interface.
#[derive(Parser)]
#[command(about = "Print whether a verified release report affects benchmark-action tool pins")]
struct Args {
    /// Stage 7 report.json produced by the completed increment-versions workflow.
    #[arg(long)]
    report: PathBuf,
    /// Proposed/local action release.json; otherwise read the action repository's default branch.
    #[arg(long)]
    action_manifest: Option<PathBuf>,
    /// Explain candidate and matching package names on stderr.
    #[arg(long)]
    verbose: bool,
}

/// Connects file/GitHub input acquisition to the pure relevance decision.
#[cfg_attr(test, mutants::skip)]
fn execute(args: Args) -> Result<bool, AppError> {
    let report = read_input(&args.report)?;
    pairing_needed(
        &report,
        || match args.action_manifest {
            Some(path) => read_input(&path),
            None => published_manifest(),
        },
        |message| {
            if args.verbose {
                eprintln!("[benchmark-action-check] {message}");
            }
        },
    )
}

/// Reads caller-selected captured evidence without refreshing or rewriting it.
#[cfg_attr(test, mutants::skip)]
fn read_input(path: &Path) -> Result<String, AppError> {
    fs::read_to_string(path)
        .map_err(|error| ReadFailed::caused_by(path.to_path_buf(), error).into())
}

/// Fetches only the authoritative manifest, not the action repository or its PR history.
#[cfg_attr(test, mutants::skip)]
fn published_manifest() -> Result<String, AppError> {
    // The action repository documents release.json as its pinned-tool authority.
    const MANIFEST_ENDPOINT: &str =
        "repos/folo-rs/cargo-bench-history-action/contents/release.json";
    let output = Command::new("gh")
        .args([
            "api",
            MANIFEST_ENDPOINT,
            "--header",
            "Accept: application/vnd.github.raw+json",
        ])
        .output()
        .map_err(ManifestLookup::caused_by)?;
    if !output.status.success() {
        return Err(
            ManifestUnavailable::new(String::from_utf8_lossy(&output.stderr).into_owned()).into(),
        );
    }
    String::from_utf8(output.stdout).map_err(|error| ManifestLookup::caused_by(error).into())
}

/// Records the concrete file whose evidence could not be read.
#[ohno::error]
#[display("Cannot read {}", path.display())]
struct ReadFailed {
    path: PathBuf,
}

/// Retains a process or encoding failure from the GitHub manifest lookup.
#[ohno::error]
#[display("Cannot read the benchmark-action release manifest")]
struct ManifestLookup;

/// A missing/bootstrap manifest is an explicit decision blocker, never an unrelated-change result.
#[ohno::error]
#[display(
    "Action manifest lookup failed. For bootstrap or proposed pins, supply --action-manifest PATH. {diagnostic}"
)]
struct ManifestUnavailable {
    diagnostic: String,
}
