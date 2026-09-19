use std::num::NonZero;
use std::path::Path;

use cbh_config::rebase;
use ohno::AppError;

use crate::action::environment::Environment;
use crate::action::errors::InvalidInput;
use crate::action::inputs::{ActionCommand, Inputs, PublishState, Sink};
use crate::cli::{
    Command, CommentReportArgs, Conclusion, FailedArgs, NoDataArgs, PendingArgs, ReportArgs,
    ResultArgs, RunArgs,
};
use crate::identity::validate_run_url;
use crate::model::{Instance, Repository};
use crate::operations::Context;

pub(crate) fn publication(
    inputs: &Inputs,
    cwd: &Path,
    instance: Instance,
    environment: &Environment,
) -> Result<(Command, Context), AppError> {
    let context = Context {
        repository: environment.repository()?,
        instance,
        verbose: true,
    };
    let run_id = positive(
        "run-id",
        inputs
            .get("run-id")
            .or_else(|| environment.value("GITHUB_RUN_ID")),
    )?;
    if inputs.command == ActionCommand::Alert {
        return Ok((
            Command::Alert {
                run_id,
                run_url: run_url(inputs, environment, &context.repository, run_id)?,
            },
            context,
        ));
    }
    let run = RunArgs {
        run_id,
        run_attempt: positive(
            "run-attempt",
            inputs
                .get("run-attempt")
                .or_else(|| environment.value("GITHUB_RUN_ATTEMPT")),
        )?,
    };
    // Inputs owns action-specific group validation. Construct the existing typed arguments
    // directly so the lifecycle dispatcher and evidence gates remain the only publishers.
    let command = match inputs.command {
        ActionCommand::Publish(Sink::Comment, state) => {
            let pull_request = match inputs.get("pr-number") {
                Some(value) => positive("pr-number", Some(value))?,
                None => environment.pull_request().ok_or_else(|| {
                    InvalidInput::new("pr-number", "required without a pull-request event")
                })?,
            };
            match state {
                PublishState::Findings => Command::PublishCommentFindings(CommentReportArgs {
                    pull_request,
                    packages: inputs.required("packages")?.to_owned(),
                    report: report(inputs, cwd, run)?,
                }),
                PublishState::Clean => Command::PublishCommentClean(CommentReportArgs {
                    pull_request,
                    packages: inputs.required("packages")?.to_owned(),
                    report: report(inputs, cwd, run)?,
                }),
                PublishState::Preflight => Command::PublishCommentPreflight {
                    pull_request,
                    packages: inputs.required("packages")?.to_owned(),
                    pending: pending(inputs, environment, run)?,
                },
                PublishState::NoData => Command::PublishCommentNoData {
                    pull_request,
                    packages: inputs.get("packages").map(str::to_owned),
                    data: no_data(inputs, cwd, environment, run)?,
                },
                PublishState::Failed => Command::PublishCommentFailed {
                    pull_request,
                    failed: failed(inputs, environment, &context.repository, run)?,
                },
            }
        }
        ActionCommand::Publish(Sink::Issue, state) => match state {
            PublishState::Findings => Command::PublishIssueFindings(report(inputs, cwd, run)?),
            PublishState::Clean => Command::PublishIssueClean(report(inputs, cwd, run)?),
            PublishState::Preflight => {
                Command::PublishIssuePreflight(pending(inputs, environment, run)?)
            }
            PublishState::NoData => {
                Command::PublishIssueNoData(no_data(inputs, cwd, environment, run)?)
            }
            PublishState::Failed => {
                Command::PublishIssueFailed(failed(inputs, environment, &context.repository, run)?)
            }
        },
        _ => return Err(InvalidInput::new("command", "expected a publication command").into()),
    };
    Ok((command, context))
}

fn positive(key: &str, value: Option<&str>) -> Result<NonZero<u64>, AppError> {
    value
        .ok_or_else(|| InvalidInput::new(key, "required without known Actions execution context"))?
        .parse()
        .map_err(|error| InvalidInput::caused_by(key, "expected a positive integer", error).into())
}

fn report(inputs: &Inputs, cwd: &Path, run: RunArgs) -> Result<ReportArgs, AppError> {
    Ok(ReportArgs {
        run,
        body_file: rebase(cwd, inputs.required("body-file")?.into()),
        analyzed_sha: inputs.required("analyzed-sha")?.parse()?,
        evidence: ResultArgs {
            report_file: rebase(cwd, inputs.required("report-file")?.into()),
            expected_platforms: inputs.required("expected-platforms")?.to_owned(),
            completed_platforms: inputs.required("completed-platforms")?.to_owned(),
        },
        artifact_url: inputs.get("artifact-url").map(str::to_owned),
    })
}

fn pending(
    inputs: &Inputs,
    environment: &Environment,
    run: RunArgs,
) -> Result<PendingArgs, AppError> {
    Ok(PendingArgs {
        run,
        head: inputs
            .get("head")
            .or_else(|| environment.head())
            .ok_or_else(|| InvalidInput::new("head", "required without a known event head"))?
            .parse()?,
    })
}

fn no_data(
    inputs: &Inputs,
    cwd: &Path,
    environment: &Environment,
    run: RunArgs,
) -> Result<NoDataArgs, AppError> {
    if inputs.boolean("empty-scope", false)? {
        return Ok(NoDataArgs {
            run,
            empty_scope: true,
            head: Some(pending(inputs, environment, run)?.head),
            body_file: None,
            analyzed_sha: None,
            report_file: None,
            expected_platforms: None,
            completed_platforms: None,
            artifact_url: None,
        });
    }
    let report = report(inputs, cwd, run)?;
    Ok(NoDataArgs {
        run,
        empty_scope: false,
        head: None,
        body_file: Some(report.body_file),
        analyzed_sha: Some(report.analyzed_sha),
        report_file: Some(report.evidence.report_file),
        expected_platforms: Some(report.evidence.expected_platforms),
        completed_platforms: Some(report.evidence.completed_platforms),
        artifact_url: report.artifact_url,
    })
}

fn failed(
    inputs: &Inputs,
    environment: &Environment,
    repository: &Repository,
    run: RunArgs,
) -> Result<FailedArgs, AppError> {
    let conclusion = match inputs.required("conclusion")? {
        "failure" => Conclusion::Failure,
        "cancelled" => Conclusion::Cancelled,
        _ => return Err(InvalidInput::new("conclusion", "expected failure or cancelled").into()),
    };
    Ok(FailedArgs {
        pending: pending(inputs, environment, run)?,
        run_url: run_url(inputs, environment, repository, run.run_id)?,
        conclusion,
    })
}

fn run_url(
    inputs: &Inputs,
    environment: &Environment,
    repository: &Repository,
    run: NonZero<u64>,
) -> Result<String, AppError> {
    let url = match inputs.get("run-url") {
        Some(url) => url.to_owned(),
        None => format!(
            "{}/{repository}/actions/runs/{run}",
            environment
                .value("GITHUB_SERVER_URL")
                .ok_or_else(|| InvalidInput::new("run-url", "required without GITHUB_SERVER_URL"))?
                .trim_end_matches('/')
        ),
    };
    validate_run_url(repository, run, &url)?;
    Ok(url)
}
