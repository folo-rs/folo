use ohno::AppError;

use crate::cli::{FailedArgs, PendingArgs};
use crate::github::GitHub;
use crate::identity::validate_run_url;
use crate::lifecycle::{
    InvalidPublication, NoData, Report, compare_or_unknown, find_comment, superseded, write_comment,
};
use crate::operations::{Context, note};
use crate::result::{AnalysisMode, PublicationState};
use crate::{marker, message};

pub(crate) async fn comment_preflight(
    github: &impl GitHub,
    context: &Context,
    pull_request: u64,
    packages: &str,
    owner: &PendingArgs,
) -> Result<(), AppError> {
    require_packages(packages)?;
    let existing = find_comment(github, context, pull_request).await?;
    let live = github
        .pull_request_head(&context.repository, pull_request)
        .await?;
    if live != owner.head {
        note(
            context,
            "the PR advanced beyond the frozen head; preflight is obsolete",
        );
        return Ok(());
    }
    let mut body = message::pr_in_progress(&context.instance, packages, owner);
    if let Some(comment) = &existing {
        if marker::find_owner(&comment.body, &context.instance)
            .is_some_and(|previous| superseded(&previous, owner))
        {
            note(
                context,
                "a later attempt of this run owns the comment; preserving it",
            );
            return Ok(());
        }
        if !message::is_in_progress(&comment.body, &context.instance)
            && !message::is_terminal_note(&comment.body, &context.instance)
        {
            let analyzed = marker::find_analyzed_sha(&comment.body, &context.instance);
            if analyzed.as_ref() == Some(&live) {
                return Ok(());
            }
            let distance = match analyzed {
                Some(analyzed) => {
                    compare_or_unknown(github, context, &analyzed, &live)
                        .await
                        .ahead_by
                }
                None => None,
            };
            body = message::insert_stale_banner(
                &comment.body,
                &context.instance,
                &message::stale_warning(distance),
            );
        }
    }
    write_comment(github, context, pull_request, existing, &body).await
}

pub(crate) async fn comment_report(
    github: &impl GitHub,
    context: &Context,
    pull_request: u64,
    packages: &str,
    report: &Report,
    state: PublicationState,
) -> Result<(), AppError> {
    require_packages(packages)?;
    report.validate(AnalysisMode::Branch, state)?;
    let existing = find_comment(github, context, pull_request).await?;
    if existing
        .as_ref()
        .and_then(|comment| marker::find_owner(&comment.body, &context.instance))
        .is_some_and(|owner| superseded(&owner, &report.owner))
    {
        note(
            context,
            "the comment belongs to a later attempt of this run; preserving it",
        );
        return Ok(());
    }
    let mut body = message::pr_result(
        &context.instance,
        &report.owner,
        &report.evidence,
        packages,
        &report.summary,
        report.artifact_url.as_deref(),
    );
    match github
        .pull_request_head(&context.repository, pull_request)
        .await
    {
        Ok(live) if live != report.owner.head => {
            if existing.as_ref().is_some_and(|comment| {
                marker::find_owner(&comment.body, &context.instance)
                    .is_some_and(|owner| owner.head == live)
            }) {
                note(
                    context,
                    "the existing comment belongs to the live head; preserving its state",
                );
                return Ok(());
            }
            let distance = compare_or_unknown(github, context, &report.owner.head, &live)
                .await
                .ahead_by;
            body = message::insert_stale_banner(
                &body,
                &context.instance,
                &message::stale_warning(distance),
            );
        }
        Ok(_) => {}
        Err(error) => {
            eprintln!("Could not verify report freshness: {error}");
            body = message::insert_stale_banner(
                &body,
                &context.instance,
                &message::freshness_unverified("Benchmark result"),
            );
        }
    }
    write_comment(github, context, pull_request, existing, &body).await
}

pub(crate) async fn comment_no_data(
    github: &impl GitHub,
    context: &Context,
    pull_request: u64,
    packages: Option<&str>,
    data: &NoData,
) -> Result<(), AppError> {
    data.validate(AnalysisMode::Branch)?;
    match data {
        NoData::Report(report) => {
            let packages = packages.ok_or_else(InvalidPublication::new)?;
            comment_report(
                github,
                context,
                pull_request,
                packages,
                report,
                PublicationState::NoData,
            )
            .await
        }
        NoData::Empty(owner) => {
            if packages.is_some() {
                return Err(InvalidPublication::new().into());
            }
            let existing = find_comment(github, context, pull_request).await?;
            let live = github
                .pull_request_head(&context.repository, pull_request)
                .await?;
            if live != owner.head {
                note(
                    context,
                    "the PR advanced; the empty-scope result no longer applies",
                );
                return Ok(());
            }
            if existing
                .as_ref()
                .and_then(|comment| marker::find_owner(&comment.body, &context.instance))
                .is_some_and(|previous| superseded(&previous, owner))
            {
                note(
                    context,
                    "a later attempt of this run owns the comment; preserving it",
                );
                return Ok(());
            }
            let body = message::pr_nothing_in_scope(&context.instance, owner);
            write_comment(github, context, pull_request, existing, &body).await
        }
    }
}

pub(crate) async fn comment_failed(
    github: &impl GitHub,
    context: &Context,
    pull_request: u64,
    args: &FailedArgs,
) -> Result<(), AppError> {
    validate_run_url(&context.repository, args.pending.run.run_id, &args.run_url)?;
    let Some(comment) = find_comment(github, context, pull_request).await? else {
        return Ok(());
    };
    if !message::is_in_progress(&comment.body, &context.instance)
        || marker::find_owner(&comment.body, &context.instance).as_ref() != Some(&args.pending)
    {
        note(
            context,
            "this failure owns no unfinished placeholder; completed or newer content is retained",
        );
        return Ok(());
    }
    let body = message::pr_failed(
        &context.instance,
        &args.pending,
        &args.run_url,
        args.conclusion,
    );
    write_comment(github, context, pull_request, Some(comment), &body).await
}

fn require_packages(packages: &str) -> Result<(), AppError> {
    if packages.split(',').any(|package| package.trim().is_empty()) {
        return Err(InvalidPublication::new().into());
    }
    Ok(())
}
