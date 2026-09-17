use std::env;

use ohno::AppError;
use tick::Clock;

use crate::cli::{Cli, Command, NoDataArgs, PendingArgs, ReportArgs, ResultArgs};
use crate::errors::{MissingRepositoryError, read_body_error};
use crate::github::RestGitHub;
use crate::lifecycle::{self, NoData, Report};
use crate::model::{CommitSha, Instance, Repository};
use crate::result::{AnalysisReport, Evidence, PlatformCoverage, PublicationState};
use crate::workflow;

/// Namespace and diagnostics shared by lifecycle and workflow operations.
#[derive(Clone, Debug)]
pub(crate) struct Context {
    pub(crate) repository: Repository,
    pub(crate) instance: Instance,
    pub(crate) verbose: bool,
}

/// Executes one lifecycle or workflow evidence command.
///
/// # Errors
///
/// Returns an error when input validation, local I/O or a required GitHub operation fails.
// Process wiring reads environment/files and creates the live adapters. Generic lifecycle
// functions below this boundary carry in-process policy coverage.
#[cfg_attr(test, mutants::skip)]
pub async fn run(cli: Cli) -> Result<(), AppError> {
    let repository = cli.repository();
    let instance = cli.instance();
    let verbose = cli.verbose();
    let command = match cli.into_command() {
        Command::WorkflowMatrix(args) => {
            return workflow::workflow_matrix(&instance, &args, verbose);
        }
        Command::InspectReport(args) => return workflow::inspect_report(args).await,
        command => command,
    };
    let repository = match repository {
        Some(repository) => repository,
        None => env::var("GITHUB_REPOSITORY")
            .map_err(MissingRepositoryError::caused_by)?
            .parse()?,
    };
    let context = Context {
        repository,
        instance,
        verbose,
    };
    let command = match command {
        Command::CollectionReceipt(args) => return workflow::collection_receipt(&context, args),
        command => command,
    };
    let github = RestGitHub::from_env()?;
    let clock = Clock::new_tokio();
    match command {
        Command::PrepareAnalysis(args) => workflow::prepare_analysis(&github, &context, args).await,
        Command::WorkflowMatrix(_) | Command::InspectReport(_) | Command::CollectionReceipt(_) => {
            unreachable!("offline commands return before GitHub construction")
        }
        Command::PublishIssueFindings(args) => {
            let report = load_report(args).await?;
            lifecycle::issue_report(
                &github,
                &context,
                &clock,
                &report,
                PublicationState::Findings,
            )
            .await
        }
        Command::PublishIssueClean(args) => {
            let report = load_report(args).await?;
            lifecycle::issue_report(&github, &context, &clock, &report, PublicationState::Clean)
                .await
        }
        Command::PublishIssuePreflight(args) => {
            lifecycle::issue_preflight(&github, &context, &clock, &args).await
        }
        Command::PublishIssueNoData(args) => {
            let data = load_no_data(args).await?;
            lifecycle::issue_no_data(&github, &context, &clock, &data).await
        }
        Command::PublishIssueFailed(args) => {
            lifecycle::issue_failed(&github, &context, &clock, &args).await
        }
        Command::PublishCommentFindings(args) => {
            let report = load_report(args.report).await?;
            lifecycle::comment_report(
                &github,
                &context,
                args.pull_request.get(),
                &args.packages,
                &report,
                PublicationState::Findings,
            )
            .await
        }
        Command::PublishCommentClean(args) => {
            let report = load_report(args.report).await?;
            lifecycle::comment_report(
                &github,
                &context,
                args.pull_request.get(),
                &args.packages,
                &report,
                PublicationState::Clean,
            )
            .await
        }
        Command::PublishCommentPreflight {
            pull_request,
            packages,
            pending,
        } => {
            lifecycle::comment_preflight(&github, &context, pull_request.get(), &packages, &pending)
                .await
        }
        Command::PublishCommentNoData {
            pull_request,
            packages,
            data,
        } => {
            let data = load_no_data(data).await?;
            lifecycle::comment_no_data(
                &github,
                &context,
                pull_request.get(),
                packages.as_deref(),
                &data,
            )
            .await
        }
        Command::PublishCommentFailed {
            pull_request,
            failed,
        } => lifecycle::comment_failed(&github, &context, pull_request.get(), &failed).await,
        Command::Alert { run_id, run_url } => {
            lifecycle::alert(&github, &context, run_id, &run_url).await
        }
    }
}

pub(crate) async fn load_evidence(
    args: ResultArgs,
    commit: &CommitSha,
) -> Result<Evidence, AppError> {
    let json = tokio::fs::read_to_string(&args.report_file)
        .await
        .map_err(|error| read_body_error(args.report_file, error))?;
    Ok(Evidence {
        report: AnalysisReport::parse(&json, commit)?,
        platforms: PlatformCoverage::parse(&args.expected_platforms, &args.completed_platforms)?,
    })
}

async fn load_report(args: ReportArgs) -> Result<Report, AppError> {
    let evidence = load_evidence(args.evidence, &args.analyzed_sha).await?;
    let summary = tokio::fs::read_to_string(&args.body_file)
        .await
        .map_err(|error| read_body_error(args.body_file, error))?;
    Ok(Report {
        owner: PendingArgs {
            run: args.run,
            head: args.analyzed_sha,
        },
        evidence,
        summary,
        artifact_url: args.artifact_url,
    })
}

// Clap guarantees the selected group's required fields; filesystem integration tests
// exercise this adapter without teaching the unit harness to perform real I/O.
#[cfg_attr(test, mutants::skip)]
async fn load_no_data(args: NoDataArgs) -> Result<NoData, AppError> {
    if args.empty_scope {
        return Ok(NoData::Empty(PendingArgs {
            run: args.run,
            head: args.head.expect("Clap requires head with empty-scope"),
        }));
    }
    let report = load_report(ReportArgs {
        run: args.run,
        body_file: args
            .body_file
            .expect("Clap requires body-file without empty-scope"),
        analyzed_sha: args
            .analyzed_sha
            .expect("Clap requires analyzed-sha without empty-scope"),
        evidence: ResultArgs {
            report_file: args
                .report_file
                .expect("Clap requires report-file without empty-scope"),
            expected_platforms: args
                .expected_platforms
                .expect("Clap requires expected-platforms without empty-scope"),
            completed_platforms: args
                .completed_platforms
                .expect("Clap requires completed-platforms without empty-scope"),
        },
        artifact_url: args.artifact_url,
    })
    .await?;
    Ok(NoData::Report(report))
}

// Diagnostic output has no policy effect; capturing global stderr would cross the unit boundary.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn note(context: &Context, message: &str) {
    if context.verbose {
        eprintln!(
            "[cargo-bench-history-github] {}: {message}",
            context.repository
        );
    }
}
