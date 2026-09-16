use std::num::NonZero;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use ohno::AppError;

use crate::marker::CommentMarker;
use crate::migration::{MigrationOptions, UnexpectedLegacyIssueTitle, validate_legacy_title};
use crate::model::{CommitSha, Instance, Repository};
use crate::workflow::{CollectionArgs, InspectArgs, MatrixArgs, PrepareArgs};

/// GitHub lifecycle and workflow evidence helpers for `cargo-bench-history`.
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

    pub(crate) fn migration_options(&self) -> Result<MigrationOptions, AppError> {
        if let Some(title) = &self.common.legacy_issue_title {
            if !matches!(
                self.command,
                Command::IssuePreflight { .. }
                    | Command::PublishIssue { .. }
                    | Command::IssueCleanup { .. }
                    | Command::Alert { .. }
                    | Command::ResolveAlert { .. }
            ) {
                return Err(UnexpectedLegacyIssueTitle::new().into());
            }
            validate_legacy_title(title)?;
        }
        let in_progress_marker = match &self.command {
            Command::PrCommentPreflight {
                legacy_in_progress_marker,
                ..
            } => legacy_in_progress_marker.clone(),
            _ => None,
        };
        Ok(MigrationOptions {
            issue_title: self.common.legacy_issue_title.clone(),
            in_progress_marker,
        })
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

    /// Adopt one unowned bot-authored open issue with this exact title; issue commands only.
    #[arg(long)]
    legacy_issue_title: Option<String>,

    /// Emit explanatory diagnostics to standard error.
    #[arg(long)]
    verbose: bool,
}

/// One companion lifecycle or workflow evidence operation.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Prepare shared matrix, platform and collection-job identity outputs without GitHub access.
    WorkflowMatrix(MatrixArgs),
    /// Record successful collection and its actual machine key without GitHub access.
    CollectionReceipt(CollectionArgs),
    /// Reconcile all collection job attempts and prepare selected analysis inputs.
    PrepareAnalysis(PrepareArgs),
    /// Project validated analysis evidence into workflow outputs without GitHub access.
    InspectReport(InspectArgs),
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
        /// Exact HTML marker identifying an existing legacy in-progress placeholder.
        #[arg(long)]
        legacy_in_progress_marker: Option<CommentMarker>,
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
            "collection-receipt",
            "prepare-analysis",
            "inspect-report",
            "workflow-matrix",
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
    fn legacy_issue_title_is_an_explicit_issue_lifecycle_option() {
        let cli = parse(&[
            "--legacy-issue-title",
            "Legacy automation title",
            "resolve-alert",
            "--run-url",
            "https://example.test/run",
        ]);
        let options = cli.migration_options().unwrap();
        assert_eq!(
            options.issue_title.as_deref(),
            Some("Legacy automation title")
        );
        assert!(options.in_progress_marker.is_none());
    }

    #[test]
    fn legacy_pr_state_marker_is_validated_and_cannot_enable_issue_title_adoption() {
        let mut cli = parse(&[
            "--comment-marker",
            "<!-- legacy-rolling -->",
            "pr-comment-preflight",
            "--pull-request",
            "1",
            "--packages",
            "package",
            "--head",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--run-id",
            "42",
            "--legacy-in-progress-marker",
            "<!-- legacy-collecting -->",
        ]);
        let options = cli.migration_options().unwrap();
        assert!(options.issue_title.is_none());
        assert_eq!(
            options
                .in_progress_marker
                .as_ref()
                .map(CommentMarker::as_str),
            Some("<!-- legacy-collecting -->")
        );
        cli.common.legacy_issue_title = Some("Legacy issue".to_owned());
        let error = cli.migration_options().unwrap_err();
        assert!(error.find_source::<UnexpectedLegacyIssueTitle>().is_some());
    }

    #[test]
    fn legacy_in_progress_marker_uses_the_existing_html_marker_validator() {
        let error = Cli::try_parse_from([
            "cargo-bench-history-github",
            "pr-comment-preflight",
            "--pull-request",
            "1",
            "--packages",
            "package",
            "--head",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--run-id",
            "42",
            "--legacy-in-progress-marker",
            "<!-- bad\nmarker -->",
        ])
        .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::ValueValidation);
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

    #[test]
    fn preparation_parses_phase_three_paths_without_changing_lifecycle_arguments() {
        let cli = parse(&[
            "--repository",
            "folo-rs/folo",
            "--instance",
            "folo",
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
            "outputs",
            "--local-results-dir",
            "results",
        ]);
        let Command::PrepareAnalysis(args) = cli.into_command() else {
            panic!("expected prepare-analysis");
        };
        assert_eq!(args.run_id.get(), 42);
        assert_eq!(args.expected_platforms, "linux,windows");
        assert_eq!(args.receipts_dir, PathBuf::from("receipts"));
        assert_eq!(args.machine_key_dir, PathBuf::from("keys"));
        assert_eq!(args.github_output, PathBuf::from("outputs"));
        assert_eq!(args.local_results_dir, Some(PathBuf::from("results")));
    }

    #[test]
    fn receipt_run_and_attempt_must_be_positive() {
        for (run, attempt) in [("0", "1"), ("1", "0")] {
            let error = Cli::try_parse_from([
                "cargo-bench-history-github",
                "collection-receipt",
                "--run-id",
                run,
                "--run-attempt",
                attempt,
                "--head",
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "--platform",
                "linux",
                "--machine-key-file",
                "key",
                "--file",
                "receipt.json",
            ])
            .unwrap_err();
            assert_eq!(error.kind(), ErrorKind::ValueValidation);
        }
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
