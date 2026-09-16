use std::num::NonZero;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::marker::CommentMarker;
use crate::model::{CommitSha, Instance, Repository};

/// GitHub lifecycle operations for `cargo-bench-history` reports.
#[derive(Debug, Parser)]
#[command(version, about)]
pub struct Cli {
    #[command(flatten)]
    common: CommonArgs,

    #[command(subcommand)]
    command: Command,
}

impl Cli {
    pub(crate) fn repository(&self) -> Option<Repository> {
        self.common.repository.clone()
    }

    pub(crate) fn instance(&self) -> Instance {
        self.common.instance.clone()
    }

    pub(crate) fn verbose(&self) -> bool {
        self.common.verbose
    }

    pub(crate) fn comment_marker(&self) -> Option<CommentMarker> {
        self.common.comment_marker.clone()
    }

    pub(crate) fn into_command(self) -> Command {
        self.command
    }
}

/// Arguments shared by every lifecycle operation.
#[derive(Args, Debug)]
struct CommonArgs {
    /// Repository in `owner/name` form; defaults to `GITHUB_REPOSITORY`.
    #[arg(long)]
    repository: Option<Repository>,

    /// Namespace separating independent action instances.
    #[arg(long, default_value = "default")]
    instance: Instance,

    /// Exact HTML marker for an existing rolling PR comment; defaults to the instance marker.
    #[arg(long)]
    comment_marker: Option<CommentMarker>,

    /// Emit explanatory diagnostics to standard error.
    #[arg(long)]
    verbose: bool,
}

/// One semantic GitHub lifecycle operation.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Mark an open regression issue stale before a new history run.
    IssuePreflight {
        /// Commit the new run is analyzing.
        #[arg(long)]
        head: CommitSha,
    },
    /// Create or update the rolling regression issue.
    PublishIssue {
        /// Displayed issue title.
        #[arg(long)]
        title: String,
        /// Markdown summary rendered by cargo-bench-history.
        #[arg(long)]
        body_file: PathBuf,
        /// Commit the summary describes.
        #[arg(long)]
        analyzed_sha: CommitSha,
        #[command(flatten)]
        evidence: ResultArgs,
        /// URL of the complete report artifact.
        #[arg(long)]
        artifact_url: Option<String>,
        /// Repository-specific introductory sentence.
        #[arg(long)]
        intro: Option<String>,
        /// Repository-specific report-reading documentation.
        #[arg(long)]
        docs_url: Option<String>,
    },
    /// Replace a recovered regression issue with an all-clear state.
    IssueCleanup {
        /// Commit that analyzed cleanly.
        #[arg(long)]
        clean_commit: CommitSha,
        #[command(flatten)]
        evidence: ResultArgs,
        /// Close the issue after writing the all-clear state.
        #[arg(long)]
        auto_close: bool,
        /// Repository-specific introductory sentence.
        #[arg(long)]
        intro: Option<String>,
        /// Repository-specific report-reading documentation.
        #[arg(long)]
        docs_url: Option<String>,
    },
    /// Create or update the rolling automation-failure issue.
    Alert {
        /// Displayed issue title.
        #[arg(long)]
        title: String,
        /// URL of the failed workflow run.
        #[arg(long)]
        run_url: String,
        /// Repository-specific introductory sentence.
        #[arg(long)]
        intro: Option<String>,
        /// Repository-specific runbook.
        #[arg(long)]
        docs_url: Option<String>,
    },
    /// Close the rolling automation-failure issue.
    ResolveAlert {
        /// URL of the successful workflow run.
        #[arg(long)]
        run_url: String,
    },
    /// Seed or mark stale the rolling pull-request comment.
    PrCommentPreflight {
        /// Pull-request number.
        #[arg(long)]
        pull_request: NonZero<u64>,
        /// Comma-separated benchmarked packages.
        #[arg(long)]
        packages: String,
        /// Frozen PR head this run will measure.
        #[arg(long)]
        head: CommitSha,
        /// Workflow run that owns the in-progress placeholder.
        #[arg(long)]
        run_id: NonZero<u64>,
    },
    /// Create or update the rolling pull-request results comment.
    PublishPrComment {
        /// Pull-request number.
        #[arg(long)]
        pull_request: NonZero<u64>,
        /// Commit the summary describes.
        #[arg(long)]
        analyzed_sha: CommitSha,
        #[command(flatten)]
        evidence: ResultArgs,
        /// Markdown summary rendered by cargo-bench-history.
        #[arg(long)]
        body_file: PathBuf,
        /// Comma-separated benchmarked packages.
        #[arg(long)]
        packages: String,
        /// URL of the complete report artifact.
        #[arg(long)]
        artifact_url: Option<String>,
        /// Repository-specific introductory sentence.
        #[arg(long)]
        intro: Option<String>,
        /// Repository-specific report-reading documentation.
        #[arg(long)]
        docs_url: Option<String>,
    },
    /// Replace or remove a rolling comment when nothing benchmarkable changed.
    PrCommentCleanup {
        /// Pull-request number.
        #[arg(long)]
        pull_request: NonZero<u64>,
        /// Frozen PR head whose package selection was empty.
        #[arg(long)]
        head: CommitSha,
        /// Delete instead of leaving the standard explanatory note.
        #[arg(long)]
        delete: bool,
    },
    /// Retire an in-progress placeholder after a genuine workflow failure.
    PrCommentFinalize {
        /// Pull-request number.
        #[arg(long)]
        pull_request: NonZero<u64>,
        /// URL of the failed workflow run.
        #[arg(long)]
        run_url: String,
        /// Frozen PR head whose benchmarking failed.
        #[arg(long)]
        head: CommitSha,
        /// Workflow run that owns the failed placeholder.
        #[arg(long)]
        run_id: NonZero<u64>,
    },
}

/// Structured analysis and collection facts shared by publication and all-clear.
#[derive(Args, Debug)]
pub(crate) struct ResultArgs {
    /// JSON report from the same analysis pass as the Markdown body.
    #[arg(long)]
    pub(crate) report_file: PathBuf,
    /// Comma-separated identifiers of every requested matrix platform.
    #[arg(long)]
    pub(crate) expected_platforms: String,
    /// Comma-separated identifiers of platforms whose collection completed successfully.
    #[arg(long)]
    pub(crate) completed_platforms: String,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use clap::CommandFactory as _;
    use clap::error::ErrorKind;
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(Cli: Send, Sync, Unpin, UnwindSafe, RefUnwindSafe);

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(
            std::iter::once("cargo-bench-history-github").chain(args.iter().copied()),
        )
        .unwrap()
    }

    #[test]
    fn every_subcommand_is_present_in_help() {
        let help = Cli::command().render_long_help().to_string();
        for name in [
            "issue-preflight",
            "publish-issue",
            "issue-cleanup",
            "alert",
            "resolve-alert",
            "pr-comment-preflight",
            "publish-pr-comment",
            "pr-comment-cleanup",
            "pr-comment-finalize",
        ] {
            assert!(help.contains(name), "{help}");
        }
    }

    #[test]
    fn common_arguments_apply_to_every_command() {
        let cli = parse(&[
            "--repository",
            "folo-rs/folo",
            "--instance",
            "nightly",
            "--verbose",
            "resolve-alert",
            "--run-url",
            "https://example.test/run",
        ]);
        assert_eq!(
            cli.repository()
                .as_ref()
                .map(ToString::to_string)
                .as_deref(),
            Some("folo-rs/folo")
        );
        assert_eq!(cli.instance().as_str(), "nightly");
        assert!(cli.verbose());

        let cli = parse(&[
            "--repository",
            "folo-rs/folo",
            "resolve-alert",
            "--run-url",
            "https://example.test/run",
        ]);
        assert!(!cli.verbose());
    }

    #[test]
    fn pull_request_number_must_be_nonzero() {
        let error = Cli::try_parse_from([
            "cargo-bench-history-github",
            "pr-comment-cleanup",
            "--pull-request",
            "0",
            "--head",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ])
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ValueValidation);
    }

    fn publication_args() -> Vec<&'static str> {
        vec![
            "cargo-bench-history-github",
            "publish-pr-comment",
            "--pull-request",
            "1",
            "--analyzed-sha",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--body-file",
            "summary.md",
            "--packages",
            "foo",
            "--report-file",
            "report.json",
            "--expected-platforms",
            "linux,windows",
            "--completed-platforms",
            "linux",
        ]
    }

    #[test]
    fn publication_carries_report_and_collection_evidence() {
        let cli = Cli::try_parse_from(publication_args()).unwrap();
        let Command::PublishPrComment { evidence, .. } = cli.into_command() else {
            panic!("expected publish-pr-comment");
        };
        assert_eq!(evidence.report_file, PathBuf::from("report.json"));
        assert_eq!(evidence.expected_platforms, "linux,windows");
        assert_eq!(evidence.completed_platforms, "linux");
    }

    fn assert_publication_requires(missing: &str) {
        let mut reduced = Vec::new();
        let mut args = publication_args().into_iter();
        while let Some(arg) = args.next() {
            if arg == missing {
                args.next();
            } else {
                reduced.push(arg);
            }
        }
        let error = Cli::try_parse_from(reduced).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn publication_requires_report() {
        assert_publication_requires("--report-file");
    }

    #[test]
    fn publication_requires_expected_platforms() {
        assert_publication_requires("--expected-platforms");
    }

    #[test]
    fn publication_requires_completed_platforms() {
        assert_publication_requires("--completed-platforms");
    }

    #[test]
    fn cleanup_requires_report_evidence_and_defaults_to_leaving_the_issue_open() {
        let cli = parse(&[
            "issue-cleanup",
            "--clean-commit",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--report-file",
            "report.json",
            "--expected-platforms",
            "linux",
            "--completed-platforms",
            "linux",
        ]);
        let Command::IssueCleanup {
            auto_close,
            evidence,
            ..
        } = cli.into_command()
        else {
            panic!("expected issue-cleanup");
        };
        assert!(!auto_close);
        assert_eq!(evidence.completed_platforms, "linux");
    }

    #[test]
    fn cleanup_requires_report_evidence() {
        let error = Cli::try_parse_from([
            "cargo-bench-history-github",
            "issue-cleanup",
            "--clean-commit",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        ])
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn preflight_carries_the_run_that_owns_the_placeholder() {
        let cli = parse(&[
            "--comment-marker",
            "<!-- team-performance -->",
            "pr-comment-preflight",
            "--pull-request",
            "1",
            "--packages",
            "foo",
            "--head",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--run-id",
            "123",
        ]);
        assert_eq!(
            cli.comment_marker().as_ref().map(CommentMarker::as_str),
            Some("<!-- team-performance -->")
        );
        let Command::PrCommentPreflight { head, run_id, .. } = cli.into_command() else {
            panic!("expected preflight");
        };
        assert_eq!(run_id.get(), 123);
        assert_eq!(head.as_str(), "a".repeat(40));
    }
}
