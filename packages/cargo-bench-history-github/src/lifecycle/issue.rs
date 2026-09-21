use std::num::NonZero;

use ohno::AppError;
use tick::Clock;

use crate::cli::{FailedArgs, PendingArgs};
use crate::github::{GitHub, Issue};
use crate::identity::{IssueIdentity, validate_run_url};
use crate::lifecycle::{
    AnnotationState, Inconclusive, IssueBody, Report, annotate, compare_or_unknown, create_issue,
    find_issue, superseded,
};
use crate::message;
use crate::operations::{Context, note};
use crate::result::{AnalysisMode, PublicationState};

/// Publishes eligible history findings or all-clear while preserving newer owned issue state.
///
/// This is the only rolling operation that may create an issue, and only for findings.
/// Validation and replacement authority are established before a body mutation is attempted.
pub(crate) async fn issue_report(
    github: &impl GitHub,
    context: &Context,
    clock: &Clock,
    report: &Report,
    state: PublicationState,
) -> Result<(), AppError> {
    let state = report.validate(AnalysisMode::History, state)?;
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
        state,
        &report.summary,
        report.artifact_url.as_deref(),
    );
    match existing {
        Some(issue) => update(github, context, clock, &issue, &body).await,
        None => create_issue(github, context, &identity, &identity.title(clock)?, &body).await,
    }
}

/// Records pending history work on an existing issue without discarding its retained report.
///
/// Both the report head and any later annotation participate in freshness protection.
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
            "a later attempt of this run owns the issue annotation; preserving it",
        );
        return Ok(());
    }
    // A retained report and a later annotation can protect different heads. Preflight
    // preserves either one proven newer; unknown order can only qualify retained content.
    // Ref: docs/implementation.md, Evidence and state transitions.
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

/// Explains unproven recovery while retaining the issue's previous report and attribution.
///
/// Explicit empty scope and inconclusive analysis share annotation ownership, not report
/// replacement. Work at an already-pending head can retire that pending status.
pub(crate) async fn issue_inconclusive(
    github: &impl GitHub,
    context: &Context,
    clock: &Clock,
    data: &Inconclusive,
) -> Result<(), AppError> {
    let state = data.validate(AnalysisMode::History)?;
    let identity = IssueIdentity::Rolling(context.instance.clone());
    let Some(issue) = find_issue(github, context, &identity).await? else {
        note(
            context,
            "no open rolling issue exists; validated inconclusive publication is a no-op",
        );
        return Ok(());
    };
    let parsed = IssueBody::parse(&issue.body, &context.instance)?;
    if superseded(parsed.latest_owner(), data.owner()) {
        note(
            context,
            "a later attempt of this run owns the issue annotation; preserving it",
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
    let mut report = parsed.report.to_owned();
    if !pending_head && parsed.commit != data.owner().head {
        // Preflight may have failed. Qualify the retained report here unless this head
        // already has an annotation whose existing staleness must be preserved.
        let comparison =
            compare_or_unknown(github, context, &parsed.commit, &data.owner().head).await;
        report = message::insert_stale_banner(
            &report,
            &context.instance,
            &message::stale_warning(comparison.ahead_by),
        );
    }
    let body = annotate(
        &report,
        &context.instance,
        data.owner(),
        state.marker_value(),
        &data.details(),
    );
    update(github, context, clock, &issue, &body).await
}

/// Replaces this run's pending issue annotation with a failure or cancellation notice.
///
/// The previous report remains intact; absence, terminal state and another owner's annotation
/// are not reasons to create or overwrite a failure record here.
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

/// Creates at most one independently discovered failure alert for this project and run.
///
/// Existing open or human-closed alerts retain their content and disposition. This operation
/// does not mutate the rolling regression issue.
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

/// Updates title and body together only when lifecycle composition changes the body.
///
/// Capturing the UTC title date here keeps retries consistent and no-op dates unchanged.
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

/// Establishes replacement authority across both retained-report and annotation ownership.
///
/// Same-commit arrivals remain serializable; different commits need forward evidence, and
/// attempt precedence applies only within the same run.
async fn may_replace(
    github: &impl GitHub,
    context: &Context,
    existing: &IssueBody<'_>,
    incoming: &PendingArgs,
) -> bool {
    if superseded(existing.latest_owner(), incoming) {
        note(
            context,
            "a later attempt of this run owns the issue; preserving its report and annotation",
        );
        return false;
    }
    // Replacement must advance both protected heads, not merely the retained report.
    // A forward comparison proves ancestry; unknown or reverse order cannot authorize it.
    // Ref: docs/implementation.md, Evidence and state transitions.
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
