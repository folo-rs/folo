use std::num::NonZero;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::action::{ActionArgs, PrepareWorkflowArgs};
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

    pub(crate) fn into_command(self) -> Command {
        self.command
    }
}

/// Common namespace and diagnostic inputs; offline commands need no repository.
#[derive(Args, Debug)]
struct CommonArgs {
    /// Repository in owner/name form; defaults to `GITHUB_REPOSITORY`.
    #[arg(long)]
    repository: Option<Repository>,
    /// Internal namespace derived from the configured project ID.
    #[arg(long, default_value = "default")]
    instance: Instance,
    /// Emit explanatory diagnostics to standard error.
    #[arg(long)]
    verbose: bool,
}

/// State-specific entry points prevent callers from choosing unsupported verdicts.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Execute a root-action invocation after binary installation.
    Action(ActionArgs),
    /// Freeze configuration, platform matrix and benchmark scope without GitHub access.
    PrepareWorkflow(PrepareWorkflowArgs),
    /// Prepare matrix and collection-job identities without GitHub access.
    WorkflowMatrix(MatrixArgs),
    /// Record successful collection and its actual machine key.
    CollectionReceipt(CollectionArgs),
    /// Reconcile collection attempts and prepare successful machine-key inputs.
    PrepareAnalysis(PrepareArgs),
    /// Project validated analysis evidence into workflow outputs.
    InspectReport(InspectArgs),
    /// Publish findings, including qualified partial findings.
    PublishCommentFindings(CommentReportArgs),
    /// Publish a completely covered clean analysis.
    PublishCommentClean(CommentReportArgs),
    /// Seed a placeholder or qualify an existing report as stale.
    PublishCommentPreflight {
        #[arg(long)]
        pull_request: NonZero<u64>,
        #[arg(long, value_parser = package_list)]
        packages: String,
        #[command(flatten)]
        pending: PendingArgs,
    },
    /// Explain empty scope or a successful analysis without a complete verdict.
    PublishCommentInconclusive {
        #[arg(long)]
        pull_request: NonZero<u64>,
        #[arg(long, required_unless_present = "empty_scope", conflicts_with = "empty_scope",
            value_parser = package_list)]
        packages: Option<String>,
        #[command(flatten)]
        data: InconclusiveArgs,
    },
    /// Retire only this run's unfinished placeholder.
    PublishCommentFailed {
        #[arg(long)]
        pull_request: NonZero<u64>,
        #[command(flatten)]
        failed: FailedArgs,
    },
    /// Create or update the rolling findings issue.
    PublishIssueFindings(ReportArgs),
    /// Publish all-clear to an existing issue, leaving it open.
    PublishIssueClean(ReportArgs),
    /// Mark an existing issue stale and record the pending run.
    PublishIssuePreflight(PendingArgs),
    /// Annotate an existing issue without replacing its previous report.
    PublishIssueInconclusive(InconclusiveArgs),
    /// Retire only this run's pending annotation.
    PublishIssueFailed(FailedArgs),
    /// File a one-off issue for a failed workflow run.
    Alert {
        #[arg(long)]
        run_id: NonZero<u64>,
        #[arg(long)]
        run_url: String,
    },
}

/// A workflow run attempt identifies the writer independently of measured commit.
#[derive(Args, Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RunArgs {
    #[arg(long)]
    pub(crate) run_id: NonZero<u64>,
    #[arg(long)]
    pub(crate) run_attempt: NonZero<u64>,
}

/// Ownership of work against a frozen head.
#[derive(Args, Clone, Debug, Eq, PartialEq)]
pub(crate) struct PendingArgs {
    #[command(flatten)]
    pub(crate) run: RunArgs,
    #[arg(long)]
    pub(crate) head: CommitSha,
}

/// Report inputs shared by every report-bearing publication.
#[derive(Args, Debug)]
pub(crate) struct ReportArgs {
    #[command(flatten)]
    pub(crate) run: RunArgs,
    #[arg(long)]
    pub(crate) body_file: PathBuf,
    #[arg(long)]
    pub(crate) analyzed_sha: CommitSha,
    #[command(flatten)]
    pub(crate) evidence: ResultArgs,
    #[arg(long)]
    pub(crate) artifact_url: Option<String>,
}

/// A PR report also discloses the benchmarked package scope.
#[derive(Args, Debug)]
pub(crate) struct CommentReportArgs {
    #[arg(long)]
    pub(crate) pull_request: NonZero<u64>,
    #[arg(long, value_parser = package_list)]
    pub(crate) packages: String,
    #[command(flatten)]
    pub(crate) report: ReportArgs,
}

/// Explicit empty scope and report evidence are mutually exclusive input groups.
#[derive(Args, Debug)]
pub(crate) struct InconclusiveArgs {
    #[command(flatten)]
    pub(crate) run: RunArgs,
    #[arg(long, requires = "head")]
    pub(crate) empty_scope: bool,
    #[arg(long, requires = "empty_scope")]
    pub(crate) head: Option<CommitSha>,
    #[arg(
        long,
        required_unless_present = "empty_scope",
        conflicts_with = "empty_scope"
    )]
    pub(crate) body_file: Option<PathBuf>,
    #[arg(
        long,
        required_unless_present = "empty_scope",
        conflicts_with = "empty_scope"
    )]
    pub(crate) analyzed_sha: Option<CommitSha>,
    #[arg(
        long,
        required_unless_present = "empty_scope",
        conflicts_with = "empty_scope"
    )]
    pub(crate) report_file: Option<PathBuf>,
    #[arg(
        long,
        required_unless_present = "empty_scope",
        conflicts_with = "empty_scope"
    )]
    pub(crate) expected_platforms: Option<String>,
    #[arg(
        long,
        required_unless_present = "empty_scope",
        conflicts_with = "empty_scope"
    )]
    pub(crate) completed_platforms: Option<String>,
    #[arg(long, conflicts_with = "empty_scope")]
    pub(crate) artifact_url: Option<String>,
}

/// Failure is execution status, not a successful analysis verdict.
#[derive(Args, Debug)]
pub(crate) struct FailedArgs {
    #[command(flatten)]
    pub(crate) pending: PendingArgs,
    #[arg(long)]
    pub(crate) run_url: String,
    #[arg(long)]
    pub(crate) conclusion: Conclusion,
}

/// Terminal execution states supported by failure publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum Conclusion {
    Failure,
    Cancelled,
}

/// Structured analysis and collection facts shared by inspection and publication.
#[derive(Args, Debug)]
pub(crate) struct ResultArgs {
    #[arg(long)]
    pub(crate) report_file: PathBuf,
    #[arg(long)]
    pub(crate) expected_platforms: String,
    #[arg(long)]
    pub(crate) completed_platforms: String,
}

/// Checks the package-scope disclosure required by direct PR lifecycle commands.
fn package_list(value: &str) -> Result<String, String> {
    if value.split(',').any(|package| package.trim().is_empty()) {
        return Err("packages must be a nonempty comma-separated list".to_owned());
    }
    Ok(value.to_owned())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use clap::error::ErrorKind;
    use clap::{Command as ClapCommand, CommandFactory};
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    assert_impl_all!(Cli: Send, Sync, Unpin, UnwindSafe, RefUnwindSafe);
    assert_not_impl_any!(RunArgs: Ord, PartialOrd);

    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn args(command: &str) -> Vec<&str> {
        vec!["companion", command, "--run-id", "42", "--run-attempt", "2"]
    }

    fn report() -> Vec<&'static str> {
        vec![
            "--body-file",
            "summary.md",
            "--report-file",
            "report.json",
            "--analyzed-sha",
            SHA,
            "--expected-platforms",
            "linux,windows",
            "--completed-platforms",
            "linux",
            "--artifact-url",
            "https://example.test/report-bundle",
        ]
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "Exhaustive generated command graph; focused argument groups retain interpreter coverage."
    )]
    fn clap_definitions_are_consistent() {
        Cli::command().debug_assert();
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "Exhaustive command/flag cross-product; focused report arguments retain interpreter coverage."
    )]
    fn report_commands_require_all_evidence_and_ownership() {
        for command in [
            "publish-comment-findings",
            "publish-comment-clean",
            "publish-comment-inconclusive",
            "publish-issue-findings",
            "publish-issue-clean",
            "publish-issue-inconclusive",
        ] {
            let mut arguments = args(command);
            arguments.extend(report());
            if command.contains("comment") {
                arguments.extend(["--pull-request", "7", "--packages", "foo"]);
            }
            Cli::try_parse_from(&arguments).unwrap();
            for flag in [
                "--run-id",
                "--run-attempt",
                "--body-file",
                "--report-file",
                "--analyzed-sha",
                "--expected-platforms",
                "--completed-platforms",
            ] {
                let position = arguments.iter().position(|arg| *arg == flag).unwrap();
                let mut reduced = arguments.clone();
                reduced.drain(position..position + 2);
                assert_eq!(
                    Cli::try_parse_from(reduced).unwrap_err().kind(),
                    ErrorKind::MissingRequiredArgument
                );
            }
        }
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "Exhaustive command/flag cross-product; focused inconclusive arguments retain interpreter coverage."
    )]
    fn empty_scope_is_explicit_and_rejects_every_report_input() {
        for command in ["publish-comment-inconclusive", "publish-issue-inconclusive"] {
            let mut arguments = args(command);
            arguments.extend(["--empty-scope", "--head", SHA]);
            if command.contains("comment") {
                arguments.extend(["--pull-request", "7"]);
            }
            Cli::try_parse_from(&arguments).unwrap();
            for pair in report().as_chunks::<2>().0 {
                let mut mixed = arguments.clone();
                mixed.extend(pair);
                assert_eq!(
                    Cli::try_parse_from(mixed).unwrap_err().kind(),
                    ErrorKind::ArgumentConflict
                );
            }
        }
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "Repeated whole-CLI construction; focused pending and failed arguments retain interpreter coverage."
    )]
    fn failed_and_preflight_forms_parse_for_both_sinks() {
        for sink in ["issue", "comment"] {
            for state in ["failed", "preflight"] {
                let command = format!("publish-{sink}-{state}");
                let mut arguments = args(&command);
                arguments.extend(["--head", SHA]);
                if sink == "comment" {
                    arguments.extend(["--pull-request", "7"]);
                    if state == "preflight" {
                        arguments.extend(["--packages", "foo,bar"]);
                    }
                }
                if state == "failed" {
                    arguments.extend([
                        "--run-url",
                        "https://github.com/o/r/actions/runs/42",
                        "--conclusion",
                        "cancelled",
                    ]);
                }
                Cli::try_parse_from(arguments).unwrap();
            }
        }
    }

    #[test]
    fn alert_has_run_but_no_attempt_and_common_options_are_retained() {
        let cli = Cli::try_parse_from([
            "companion",
            "--repository",
            "folo-rs/folo",
            "--instance",
            "project",
            "--verbose",
            "alert",
            "--run-id",
            "42",
            "--run-url",
            "https://github.com/folo-rs/folo/actions/runs/42",
        ])
        .unwrap();
        assert_eq!(cli.repository().unwrap().to_string(), "folo-rs/folo");
        assert_eq!(cli.instance().as_str(), "project");
        assert!(cli.verbose());
        assert!(matches!(cli.into_command(), Command::Alert { .. }));
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "Exhaustive removed-command and option catalogue; native CLI coverage checks every rejection."
    )]
    fn removed_commands_and_cosmetic_options_are_not_aliases() {
        for command in [
            "publish-issue",
            "publish-pr-comment",
            "publish-comment-no-data",
            "publish-issue-no-data",
            "resolve-alert",
            "issue-cleanup",
            "pr-comment-finalize",
            "pr-comment-cleanup",
        ] {
            assert_eq!(
                Cli::try_parse_from(["companion", command])
                    .unwrap_err()
                    .kind(),
                ErrorKind::InvalidSubcommand
            );
        }
        for option in [
            "--title",
            "--intro",
            "--docs-url",
            "--auto-close",
            "--delete",
            "--legacy-in-progress-marker",
        ] {
            assert_eq!(
                Cli::try_parse_from(["companion", "publish-issue-findings", option])
                    .unwrap_err()
                    .kind(),
                ErrorKind::UnknownArgument
            );
        }
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "Exhaustive whole-CLI invalid-input matrix; focused argument groups retain interpreter coverage."
    )]
    fn numeric_ownership_and_package_scope_are_nonempty() {
        for packages in ["", "foo,", ",bar"] {
            let mut arguments = args("publish-comment-preflight");
            arguments.extend(["--head", SHA, "--pull-request", "7", "--packages", packages]);
            assert_eq!(
                Cli::try_parse_from(arguments).unwrap_err().kind(),
                ErrorKind::ValueValidation
            );
        }
        for field in ["--run-id", "--run-attempt", "--pull-request"] {
            let mut arguments = args("publish-comment-preflight");
            arguments.extend(["--head", SHA, "--pull-request", "7", "--packages", "foo"]);
            let index = arguments.iter().position(|value| *value == field).unwrap();
            *arguments.get_mut(index + 1).unwrap() = "0";
            assert_eq!(
                Cli::try_parse_from(arguments).unwrap_err().kind(),
                ErrorKind::ValueValidation
            );
        }
    }

    #[test]
    fn report_argument_group_accepts_complete_evidence() {
        let mut arguments = vec!["report", "--run-id", "42", "--run-attempt", "2"];
        arguments.extend(report());
        ReportArgs::augment_args(ClapCommand::new("report"))
            .try_get_matches_from(&arguments)
            .unwrap();
    }

    #[test]
    fn report_argument_group_requires_the_report_artifact() {
        let mut arguments = vec!["report", "--run-id", "42", "--run-attempt", "2"];
        arguments.extend(report());
        let index = arguments
            .iter()
            .position(|arg| *arg == "--report-file")
            .unwrap();
        arguments.drain(index..index + 2);
        assert_eq!(
            ReportArgs::augment_args(ClapCommand::new("report"))
                .try_get_matches_from(arguments)
                .unwrap_err()
                .kind(),
            ErrorKind::MissingRequiredArgument
        );
    }

    #[test]
    fn inconclusive_argument_group_distinguishes_empty_scope_from_missing_report() {
        let arguments = [
            "inconclusive",
            "--run-id",
            "42",
            "--run-attempt",
            "2",
            "--empty-scope",
            "--head",
            SHA,
        ];
        InconclusiveArgs::augment_args(ClapCommand::new("inconclusive"))
            .try_get_matches_from(arguments)
            .unwrap();
        assert_eq!(
            InconclusiveArgs::augment_args(ClapCommand::new("inconclusive"))
                .try_get_matches_from(arguments.into_iter().chain(["--body-file", "summary.md"]))
                .unwrap_err()
                .kind(),
            ErrorKind::ArgumentConflict
        );
    }

    #[test]
    fn failed_argument_group_accepts_cancellation_not_success() {
        let arguments = [
            "failed",
            "--run-id",
            "42",
            "--run-attempt",
            "2",
            "--head",
            SHA,
            "--run-url",
            "https://github.com/folo-rs/folo/actions/runs/42",
            "--conclusion",
        ];
        FailedArgs::augment_args(ClapCommand::new("failed"))
            .try_get_matches_from(arguments.into_iter().chain(["cancelled"]))
            .unwrap();
        assert_eq!(
            FailedArgs::augment_args(ClapCommand::new("failed"))
                .try_get_matches_from(arguments.into_iter().chain(["success"]))
                .unwrap_err()
                .kind(),
            ErrorKind::InvalidValue
        );
    }

    #[test]
    fn pending_argument_group_rejects_zero_attempts_and_scope_parser_rejects_empty_items() {
        assert_eq!(
            PendingArgs::augment_args(ClapCommand::new("pending"))
                .try_get_matches_from([
                    "pending",
                    "--run-id",
                    "42",
                    "--run-attempt",
                    "0",
                    "--head",
                    SHA
                ])
                .unwrap_err()
                .kind(),
            ErrorKind::ValueValidation
        );
        package_list("foo,bar").unwrap();
        package_list("foo,").unwrap_err();
    }
}
