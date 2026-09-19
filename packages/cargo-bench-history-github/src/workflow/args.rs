use std::num::NonZero;
use std::path::PathBuf;

use clap::Args;

use crate::cli::ResultArgs;
use crate::model::CommitSha;

/// Platform and instance setup shared by collection jobs and later analysis.
#[derive(Args, Debug)]
pub(crate) struct MatrixArgs {
    /// Comma-separated collection platform identifiers.
    #[arg(long)]
    pub(crate) platforms: String,
    #[arg(long)]
    pub(crate) github_output: PathBuf,
}

/// File-backed proof emitted after collection and machine-key capture succeed.
#[derive(Args, Debug)]
pub(crate) struct CollectionArgs {
    #[arg(long)]
    pub(crate) run_id: NonZero<u64>,
    #[arg(long)]
    pub(crate) run_attempt: NonZero<u64>,
    #[arg(long)]
    pub(crate) head: CommitSha,
    /// Stable collection matrix identifier.
    #[arg(long)]
    pub(crate) platform: String,
    /// File containing the real machine-key command's hexadecimal fingerprint.
    #[arg(long)]
    pub(crate) machine_key_file: PathBuf,
    /// Receipt destination, uploaded as receipt.json at the artifact root.
    #[arg(long)]
    pub(crate) file: PathBuf,
}

/// Run-bound collection inputs and fresh destinations for the analysis job.
#[derive(Args, Debug)]
pub(crate) struct PrepareArgs {
    #[arg(long)]
    pub(crate) run_id: NonZero<u64>,
    #[arg(long)]
    pub(crate) head: CommitSha,
    #[arg(long)]
    pub(crate) expected_platforms: String,
    /// Download root containing one subdirectory per collection artifact.
    #[arg(long)]
    pub(crate) receipts_dir: PathBuf,
    /// Absent or empty destination for one machine-key file per successful platform.
    #[arg(long)]
    pub(crate) machine_key_dir: PathBuf,
    #[arg(long)]
    pub(crate) github_output: PathBuf,
}

/// Projection of the publication parser's evidence into analysis-job decisions.
#[derive(Args, Debug)]
pub(crate) struct InspectArgs {
    #[command(flatten)]
    pub(crate) evidence: ResultArgs,
    #[arg(long)]
    pub(crate) analyzed_sha: CommitSha,
    #[arg(long)]
    pub(crate) github_output: PathBuf,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use clap::Command;
    use clap::error::ErrorKind;

    use super::*;

    const PREPARATION_ARGS: &[&str] = &[
        "prepare-analysis",
        "--run-id",
        "42",
        "--head",
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "--expected-platforms",
        "linux,windows",
        "--receipts-dir",
        "receipts",
        "--machine-key-dir",
        "keys",
        "--github-output",
        "github-output",
    ];

    #[test]
    fn preparation_arguments_accept_receipts() {
        PrepareArgs::augment_args(Command::new("prepare-analysis"))
            .try_get_matches_from(PREPARATION_ARGS)
            .unwrap();
    }

    #[test]
    fn preparation_arguments_reject_local_results() {
        assert_eq!(
            PrepareArgs::augment_args(Command::new("prepare-analysis"))
                .try_get_matches_from(
                    PREPARATION_ARGS
                        .iter()
                        .copied()
                        .chain(["--local-results-dir", "results"])
                )
                .unwrap_err()
                .kind(),
            ErrorKind::UnknownArgument
        );
    }
}
