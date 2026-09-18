use std::num::NonZero;

use ohno::AppError;
use tick::Clock;

use crate::cli::{FailedArgs, PendingArgs};
use crate::github::{GitHub, Issue};
use crate::identity::{IssueIdentity, validate_run_url};
use crate::lifecycle::{
    AnnotationState, IssueBody, NoData, Report, annotate, compare_or_unknown, create_issue,
    find_issue, superseded,
};
use crate::message;
use crate::operations::{Context, note};
use crate::result::{AnalysisMode, PublicationState};

pub(crate) async fn issue_report(
    github: &impl GitHub,
    context: &Context,
    clock: &Clock,
    report: &Report,
    state: PublicationState,
) -> Result<(), AppError> {
    report.validate(AnalysisMode::History, state)?;
    let identity = IssueIdentity::Rolling(context.instance.clone());
    let existing = find_issue(github, context, &identity).await?;
    if let Some(issue) = &existing {
        let parsed = IssueBody::parse(&issue.body, &context.instance)?;
        if !may_replace(github, context, &parsed, &report.owner).await {
            return Ok(());
        }
    } else if state != PublicationState::Findings {
        note(
            context,
            "no open rolling issue exists; validated non-findings publication is a no-op",
        );
        return Ok(());
    }
    let body = message::regression_issue(
        &context.instance,
        &report.owner,
        &report.evidence,
        &report.summary,
        report.artifact_url.as_deref(),
    );
    match existing {
        Some(issue) => update(github, context, clock, &issue, &body).await,
        None => create_issue(github, context, &identity, &identity.title(clock)?, &body).await,
    }
}

pub(crate) async fn issue_preflight(
    github: &impl GitHub,
    context: &Context,
    clock: &Clock,
    owner: &PendingArgs,
) -> Result<(), AppError> {
    let identity = IssueIdentity::Rolling(context.instance.clone());
    let Some(issue) = find_issue(github, context, &identity).await? else {
        note(
            context,
            "no open rolling issue exists; preflight does not create an issue",
        );
        return Ok(());
    };
    let parsed = IssueBody::parse(&issue.body, &context.instance)?;
    if superseded(parsed.latest_owner(), owner) {
        note(
            context,
            "a later run attempt owns the issue annotation; preserving it",
        );
        return Ok(());
    }
    if parsed.annotation.is_some()
        && parsed.latest_owner().head != owner.head
        && compare_or_unknown(github, context, &owner.head, &parsed.latest_owner().head)
            .await
            .ahead_by
            .is_some_and(|distance| distance > 0)
    {
        note(
            context,
            "the pending head is proven newer than this preflight head; preserving its annotation",
        );
        return Ok(());
    }
    let mut report = parsed.report.to_owned();
    if parsed.commit != owner.head {
        let comparison = compare_or_unknown(github, context, &parsed.commit, &owner.head).await;
        if comparison.ahead_by.is_none()
            && compare_or_unknown(github, context, &owner.head, &parsed.commit)
                .await
                .ahead_by
                .is_some_and(|distance| distance > 0)
        {
            note(
                context,
                "the reverse comparison proves the report newer than this preflight head",
            );
            return Ok(());
        }
        report = message::insert_stale_banner(
            &report,
            &context.instance,
            &message::stale_warning(comparison.ahead_by),
        );
    }
    let body = annotate(
        &report,
        &context.instance,
        owner,
        "preflight",
        "Benchmarking is in progress; the previous report is retained.",
    );
    update(github, context, clock, &issue, &body).await
}

pub(crate) async fn issue_no_data(
    github: &impl GitHub,
    context: &Context,
    clock: &Clock,
    data: &NoData,
) -> Result<(), AppError> {
    data.validate(AnalysisMode::History)?;
    let identity = IssueIdentity::Rolling(context.instance.clone());
    let Some(issue) = find_issue(github, context, &identity).await? else {
        note(
            context,
            "no open rolling issue exists; validated no-data publication is a no-op",
        );
        return Ok(());
    };
    let parsed = IssueBody::parse(&issue.body, &context.instance)?;
    if superseded(parsed.latest_owner(), data.owner()) {
        note(
            context,
            "a later run attempt owns the issue annotation; preserving it",
        );
        return Ok(());
    }
    // Retiring work at an already-pending head does not replace the retained report.
    // Its unknown commit distance must not strand that run's in-progress annotation.
    let pending_head = parsed
        .annotation
        .as_ref()
        .is_some_and(|annotation| annotation.owner.head == data.owner().head);
    if !pending_head && !may_replace(github, context, &parsed, data.owner()).await {
        return Ok(());
    }
    let body = annotate(
        parsed.report,
        &context.instance,
        data.owner(),
        "no-data",
        &data.details(),
    );
    update(github, context, clock, &issue, &body).await
}

pub(crate) async fn issue_failed(
    github: &impl GitHub,
    context: &Context,
    clock: &Clock,
    args: &FailedArgs,
) -> Result<(), AppError> {
    validate_run_url(&context.repository, args.pending.run.run_id, &args.run_url)?;
    let identity = IssueIdentity::Rolling(context.instance.clone());
    let Some(issue) = find_issue(github, context, &identity).await? else {
        note(
            context,
            "no open rolling issue exists; failure publication does not create one",
        );
        return Ok(());
    };
    let parsed = IssueBody::parse(&issue.body, &context.instance)?;
    if !parsed.annotation.as_ref().is_some_and(|annotation| {
        annotation.state == AnnotationState::Preflight && annotation.owner == args.pending
    }) {
        note(
            context,
            "no pending annotation belongs to this exact run, attempt and head",
        );
        return Ok(());
    }
    let details = format!(
        "{}\nWorkflow run: {}",
        message::failure_notice(args.conclusion),
        args.run_url
    );
    let body = annotate(
        parsed.report,
        &context.instance,
        &args.pending,
        "failed",
        &details,
    );
    update(github, context, clock, &issue, &body).await
}

pub(crate) async fn alert(
    github: &impl GitHub,
    context: &Context,
    run_id: NonZero<u64>,
    run_url: &str,
) -> Result<(), AppError> {
    validate_run_url(&context.repository, run_id, run_url)?;
    let identity = IssueIdentity::Alert(context.instance.clone(), run_id);
    if find_issue(github, context, &identity).await?.is_some() {
        note(
            context,
            "this workflow run already has an alert; preserving its content and disposition",
        );
        return Ok(());
    }
    let body = message::failure_issue(&context.instance, run_id.get(), run_url);
    create_issue(github, context, &identity, &identity.phrase(), &body).await
}

async fn update(
    github: &impl GitHub,
    context: &Context,
    clock: &Clock,
    issue: &Issue,
    body: &str,
) -> Result<(), AppError> {
    if issue.body == body {
        return Ok(());
    }
    // Capture only for a body mutation; adapter retries reuse this exact title and body.
    let title = IssueIdentity::Rolling(context.instance.clone()).title(clock)?;
    github
        .update_issue(&context.repository, issue.number, &title, body)
        .await
}

async fn may_replace(
    github: &impl GitHub,
    context: &Context,
    existing: &IssueBody<'_>,
    incoming: &PendingArgs,
) -> bool {
    if superseded(existing.latest_owner(), incoming) {
        note(
            context,
            "a later workflow run attempt owns this issue; preserving its report and annotation",
        );
        return false;
    }
    for head in [&existing.commit, &existing.latest_owner().head] {
        if *head != incoming.head
            && !compare_or_unknown(github, context, head, &incoming.head)
                .await
                .ahead_by
                .is_some_and(|distance| distance > 0)
        {
            eprintln!("Preserving regression issue: incoming commit is not verified as newer.");
            return false;
        }
    }
    true
}
