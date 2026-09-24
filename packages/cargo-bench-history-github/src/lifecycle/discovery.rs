use std::panic::{RefUnwindSafe, UnwindSafe};

use ohno::{AppError, EnrichableExt as _};

use crate::github::{Comment, Comparison, GitHub, Issue};
use crate::identity::IssueIdentity;
use crate::lifecycle::{IssueBody, UninterpretableIssue};
use crate::marker;
use crate::model::{CommitSha, Instance, IssueKind};
use crate::operations::Context;

/// Discovers one reserved issue identity and revalidates its directly read lifecycle body.
///
/// Lifecycle callers receive absence only after complete title discovery. Search candidates
/// are locators, not authoritative body or state snapshots.
pub(crate) async fn find_issue(
    github: &impl GitHub,
    context: &Context,
    identity: &IssueIdentity,
) -> Result<Option<Issue>, AppError> {
    let candidates = github
        .search_issues(
            &context.repository,
            &identity.phrase(),
            identity.includes_closed(),
        )
        .await?;
    let mut matching = candidates
        .iter()
        .filter(|candidate| identity.matches(&candidate.title));
    let Some(candidate) = matching.next() else {
        return Ok(None);
    };
    if matching.next().is_some() {
        return Err(AmbiguousIdentity::new().into());
    }
    let issue = github
        .read_issue(&context.repository, candidate.number)
        .await?;
    if !identity.matches(&issue.title) || (!identity.includes_closed() && !issue.open) {
        return Err(ChangedIssueIdentity::new().into());
    }
    match identity {
        IssueIdentity::Rolling(_) => {
            IssueBody::parse(&issue.body, &context.instance)?;
        }
        IssueIdentity::Alert(_, run) => {
            let identity = marker::issue(&context.instance, IssueKind::FailureAlert);
            let run = marker::alert_run(&context.instance, run.get());
            if issue.body.lines().filter(|line| *line == identity).count() != 1
                || issue.body.lines().filter(|line| *line == run).count() != 1
            {
                return Err(UninterpretableIssue::new().into());
            }
        }
    }
    Ok(Some(issue))
}

/// Selects one coherent project-owned lifecycle comment within the requested PR conversation.
///
/// Ownership here is the design's reserved marker/metadata identity, not an author check.
pub(crate) async fn find_comment(
    github: &impl GitHub,
    context: &Context,
    pull_request: u64,
) -> Result<Option<Comment>, AppError> {
    let identity = marker::pr_comment(&context.instance);
    let mut comments = github
        .comments(&context.repository, pull_request)
        .await?
        .into_iter()
        .filter(|comment| comment.body.lines().any(|line| line == identity));
    let first = comments.next();
    if comments.next().is_some() {
        return Err(AmbiguousIdentity::new().into());
    }
    if let Some(comment) = &first {
        require_comment_metadata(&comment.body, &context.instance)?;
    }
    Ok(first)
}

/// Distinguishes valid notes and reports before any discovered comment can be modified.
fn require_comment_metadata(body: &str, instance: &Instance) -> Result<(), AppError> {
    let owner = marker::find_owner(body, instance).ok_or_else(UninterpretableComment::new)?;
    let identity = marker::pr_comment(instance);
    let notes = [
        marker::in_progress(instance),
        marker::empty_scope(instance),
        marker::failed(instance),
    ];
    let note_count = body
        .lines()
        .filter(|line| notes.iter().any(|note| *line == note))
        .count();
    // Ownership alone cannot distinguish a supported note/report from copied or
    // contradictory metadata. Validate before any lifecycle operation can overwrite it.
    let valid_state = match note_count {
        0 => {
            matches!(
                marker::find_state(body, instance),
                Some("findings" | "clean" | "no-data")
            ) && marker::find_analyzed_sha(body, instance).as_ref() == Some(&owner.head)
        }
        1 => {
            !marker::has_value(body, instance, "state")
                && !marker::has_value(body, instance, "analyzed-sha")
        }
        _ => false,
    };
    if !valid_state || body.lines().filter(|line| *line == identity).count() != 1 {
        return Err(UninterpretableComment::new().into());
    }
    Ok(())
}

/// Performs one issue create and reconciles an ambiguous result without another create.
///
/// Reconciliation requires the intended identity and exact content, so another publication at
/// the same title cannot be mistaken for this operation's success.
pub(crate) async fn create_issue(
    github: &impl GitHub,
    context: &Context,
    identity: &IssueIdentity,
    title: &str,
    body: &str,
) -> Result<(), AppError> {
    // Search indexing can lag a committed write. Reconciliation is bounded and never
    // authorizes another POST; it establishes only the exact intended title and body.
    const RECONCILIATION_READS: usize = 3;
    let error = match github.create_issue(&context.repository, title, body).await {
        Ok(issue) if issue.title == title && issue.body == body => return Ok(()),
        Ok(_) => return Err(ChangedIssueIdentity::new().into()),
        Err(error) => error,
    };
    for _ in 0..RECONCILIATION_READS {
        match find_issue(github, context, identity).await {
            Ok(Some(issue)) if issue.title == title && issue.body == body => return Ok(()),
            Ok(Some(_)) => return Err(error.enrich(
                "create reconciliation found different content; preserving the other publication",
            )),
            Ok(None) => {}
            Err(_) => {
                return Err(error.enrich("issue reconciliation failed after the create error"));
            }
        }
    }
    Err(error.enrich("bounded issue reconciliation could not establish that the create committed"))
}

/// Commits a lifecycle-selected comment body using update or one reconciled create.
///
/// Policy callers have already checked freshness and ownership. An identical body needs no
/// write; an ambiguous create is resolved only by finding this same intended content.
pub(crate) async fn write_comment(
    github: &impl GitHub,
    context: &Context,
    pull_request: u64,
    existing: Option<Comment>,
    body: &str,
) -> Result<(), AppError> {
    if let Some(comment) = existing {
        if comment.body == body {
            return Ok(());
        }
        return github
            .update_comment(&context.repository, comment.id, body)
            .await;
    }
    match github
        .create_comment(&context.repository, pull_request, body)
        .await
    {
        Ok(comment) if comment.body == body => Ok(()),
        Ok(_) => Err(UnexpectedCreatedComment::new().into()),
        Err(error) => match find_comment(github, context, pull_request).await {
            Ok(Some(comment)) if comment.body == body => Ok(()),
            Ok(Some(_)) => Err(error.enrich("comment reconciliation found different content")),
            Ok(None) => Err(error),
            Err(_) => Err(error.enrich("comment reconciliation failed after the create error")),
        },
    }
}

/// Obtains commit-order evidence without turning a failed comparison into a numeric distance.
///
/// Lifecycle callers use the optional distance either to qualify retained reports or to require
/// a verified forward advance before replacement.
pub(crate) async fn compare_or_unknown(
    github: &impl GitHub,
    context: &Context,
    base: &CommitSha,
    head: &CommitSha,
) -> Comparison {
    match github.compare(&context.repository, base, head).await {
        Ok(comparison) => comparison,
        Err(error) => {
            eprintln!("Could not determine report staleness distance: {error}");
            Comparison { ahead_by: None }
        }
    }
}

/// More than one exact identity cannot choose a unique mutation target.
#[ohno::error]
#[display("Multiple GitHub artifacts match the intended identity")]
struct AmbiguousIdentity;

/// Fresh direct reads, not search snapshots, govern issue identity and state.
#[ohno::error]
#[display("Issue identity or state changed during discovery")]
struct ChangedIssueIdentity;

/// Matching comments require coherent ownership and supported lifecycle metadata.
#[ohno::error]
#[display("Matching pull-request comment has uninterpretable lifecycle metadata")]
struct UninterpretableComment;

/// A successful create response must describe the intended publication.
#[ohno::error]
#[display("Created comment does not contain the requested body")]
struct UnexpectedCreatedComment;

// These immutable errors expose no mutation across unwinding.
impl UnwindSafe for AmbiguousIdentity {}
impl RefUnwindSafe for AmbiguousIdentity {}
impl UnwindSafe for ChangedIssueIdentity {}
impl RefUnwindSafe for ChangedIssueIdentity {}
impl UnwindSafe for UninterpretableComment {}
impl RefUnwindSafe for UninterpretableComment {}
impl UnwindSafe for UnexpectedCreatedComment {}
impl RefUnwindSafe for UnexpectedCreatedComment {}
