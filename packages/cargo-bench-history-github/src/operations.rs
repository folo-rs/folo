use std::env;

use ohno::{AppError, EnrichableExt as _};

use crate::cli::{Cli, Command, ResultArgs};
use crate::errors::{MissingRepositoryError, read_body_error};
use crate::github::{Comment, Comparison, GitHub, Issue, RestGitHub};
use crate::marker;
use crate::marker::{CommentMarker, UnexpectedCommentMarker};
use crate::message::{self, Envelope};
use crate::model::{CommitSha, Instance, IssueKind, Repository};
use crate::result::{AnalysisMode, AnalysisReport, Evidence, PlatformCoverage};

/// Inputs shared by every lifecycle operation.
#[derive(Clone, Debug)]
pub(crate) struct Context {
    pub(crate) repository: Repository,
    pub(crate) instance: Instance,
    pub(crate) verbose: bool,
    pub(crate) comment_marker: Option<CommentMarker>,
}

impl Context {
    fn pr_identity(&self) -> String {
        self.comment_marker.as_ref().map_or_else(
            || marker::pr_comment(&self.instance),
            |marker| marker.as_str().to_owned(),
        )
    }
}

/// Executes one GitHub lifecycle command.
///
/// # Errors
///
/// Returns an error when inputs cannot be loaded or a required GitHub operation
/// does not complete successfully.
// Process wiring constructs the live adapter and reads process environment/filesystem.
// The generic lifecycle functions it dispatches to carry the behavioral tests.
#[cfg_attr(test, mutants::skip)]
pub async fn run(cli: Cli) -> Result<(), AppError> {
    let repository = match cli.repository() {
        Some(repository) => repository,
        None => env::var("GITHUB_REPOSITORY")
            .map_err(MissingRepositoryError::caused_by)?
            .parse()?,
    };
    let context = Context {
        repository,
        instance: cli.instance(),
        verbose: cli.verbose(),
        comment_marker: cli.comment_marker(),
    };
    let command = cli.into_command();
    if context.comment_marker.is_some()
        && !matches!(
            command,
            Command::PrCommentPreflight { .. }
                | Command::PublishPrComment { .. }
                | Command::PrCommentCleanup { .. }
                | Command::PrCommentFinalize { .. }
        )
    {
        return Err(UnexpectedCommentMarker::new().into());
    }
    let github = RestGitHub::from_env()?;

    match command {
        Command::IssuePreflight { head } => issue_preflight(&github, &context, &head).await,
        Command::PublishIssue {
            title,
            body_file,
            analyzed_sha,
            evidence,
            artifact_url,
            intro,
            docs_url,
        } => {
            let body = tokio::fs::read_to_string(&body_file)
                .await
                .map_err(|error| read_body_error(body_file, error))?;
            let evidence = load_evidence(evidence, &analyzed_sha).await?;
            publish_issue(
                &github,
                &context,
                &title,
                &body,
                &evidence,
                Envelope {
                    intro: intro.as_deref(),
                    docs_url: docs_url.as_deref(),
                    artifact_url: artifact_url.as_deref(),
                },
            )
            .await
        }
        Command::IssueCleanup {
            clean_commit,
            evidence,
            auto_close,
            intro,
            docs_url,
        } => {
            let evidence = load_evidence(evidence, &clean_commit).await?;
            issue_cleanup(
                &github,
                &context,
                &evidence,
                auto_close,
                Envelope {
                    intro: intro.as_deref(),
                    docs_url: docs_url.as_deref(),
                    artifact_url: None,
                },
            )
            .await
        }
        Command::Alert {
            title,
            run_url,
            intro,
            docs_url,
        } => {
            alert(
                &github,
                &context,
                &title,
                &run_url,
                Envelope {
                    intro: intro.as_deref(),
                    docs_url: docs_url.as_deref(),
                    artifact_url: None,
                },
            )
            .await
        }
        Command::ResolveAlert { run_url } => resolve_alert(&github, &context, &run_url).await,
        Command::PrCommentPreflight {
            pull_request,
            packages,
            head,
            run_id,
        } => {
            pr_comment_preflight(
                &github,
                &context,
                pull_request.get(),
                &packages,
                &head,
                run_id.get(),
            )
            .await
        }
        Command::PublishPrComment {
            pull_request,
            analyzed_sha,
            evidence,
            body_file,
            packages,
            artifact_url,
            intro,
            docs_url,
        } => {
            let body = tokio::fs::read_to_string(&body_file)
                .await
                .map_err(|error| read_body_error(body_file, error))?;
            let evidence = load_evidence(evidence, &analyzed_sha).await?;
            publish_pr_comment(
                &github,
                &context,
                pull_request.get(),
                &evidence,
                &packages,
                &body,
                Envelope {
                    intro: intro.as_deref(),
                    docs_url: docs_url.as_deref(),
                    artifact_url: artifact_url.as_deref(),
                },
            )
            .await
        }
        Command::PrCommentCleanup {
            pull_request,
            head,
            delete,
        } => pr_comment_cleanup(&github, &context, pull_request.get(), &head, delete).await,
        Command::PrCommentFinalize {
            pull_request,
            run_url,
            head,
            run_id,
        } => {
            pr_comment_finalize(
                &github,
                &context,
                pull_request.get(),
                &run_url,
                &head,
                run_id.get(),
            )
            .await
        }
    }
}

async fn load_evidence(args: ResultArgs, commit: &CommitSha) -> Result<Evidence, AppError> {
    let json = tokio::fs::read_to_string(&args.report_file)
        .await
        .map_err(|error| read_body_error(args.report_file, error))?;
    Ok(Evidence {
        report: AnalysisReport::parse(&json, commit)?,
        platforms: PlatformCoverage::parse(&args.expected_platforms, &args.completed_platforms)?,
    })
}

pub(crate) async fn issue_preflight(
    github: &impl GitHub,
    context: &Context,
    head: &CommitSha,
) -> Result<(), AppError> {
    let identity = marker::issue(&context.instance, IssueKind::Regression);
    let Some(issue) = find_issue(github, &context.repository, &identity).await? else {
        note(
            context,
            "no rolling regression issue exists, so preflight is a no-op",
        );
        return Ok(());
    };
    let Some(analyzed) = marker::find_analyzed_sha(&issue.body, &context.instance) else {
        let body = message::insert_stale_banner(
            &issue.body,
            &context.instance,
            &message::stale_warning(None, "Findings are"),
        );
        return github
            .update_issue(&context.repository, issue.number, None, &body)
            .await;
    };
    if analyzed == *head {
        note(
            context,
            "the rolling issue already describes the current commit",
        );
        return Ok(());
    }
    let comparison = compare_or_unknown(github, context, &analyzed, head).await;
    let warning = message::stale_warning(comparison.ahead_by, "Findings are");
    let body = message::insert_stale_banner(&issue.body, &context.instance, &warning);
    github
        .update_issue(&context.repository, issue.number, None, &body)
        .await
}

pub(crate) async fn publish_issue(
    github: &impl GitHub,
    context: &Context,
    title: &str,
    summary: &str,
    evidence: &Evidence,
    envelope: Envelope<'_>,
) -> Result<(), AppError> {
    evidence.report.require_mode(AnalysisMode::History)?;
    evidence.report.require_findings()?;
    let identity = marker::issue(&context.instance, IssueKind::Regression);
    let body = message::regression_issue(&context.instance, evidence, summary, envelope);
    upsert_issue(
        github,
        context,
        title,
        &identity,
        &body,
        Some(&evidence.report.commit),
    )
    .await
}

pub(crate) async fn issue_cleanup(
    github: &impl GitHub,
    context: &Context,
    evidence: &Evidence,
    auto_close: bool,
    envelope: Envelope<'_>,
) -> Result<(), AppError> {
    evidence.report.require_mode(AnalysisMode::History)?;
    evidence.require_all_clear()?;
    let clean_commit = &evidence.report.commit;
    let identity = marker::issue(&context.instance, IssueKind::Regression);
    let Some(issue) = find_issue(github, &context.repository, &identity).await? else {
        note(
            context,
            "no rolling regression issue exists, so cleanup is a no-op",
        );
        return Ok(());
    };
    if !may_replace_issue(github, context, &issue, clean_commit).await {
        return Ok(());
    }
    let body = message::all_clear_issue(&context.instance, clean_commit, envelope);
    github
        .update_issue(&context.repository, issue.number, None, &body)
        .await?;
    if auto_close {
        github
            .close_issue(&context.repository, issue.number)
            .await?;
    }
    Ok(())
}

pub(crate) async fn alert(
    github: &impl GitHub,
    context: &Context,
    title: &str,
    run_url: &str,
    envelope: Envelope<'_>,
) -> Result<(), AppError> {
    let identity = marker::issue(&context.instance, IssueKind::FailureAlert);
    let body = message::failure_issue(&context.instance, run_url, envelope);
    upsert_issue(github, context, title, &identity, &body, None).await
}

pub(crate) async fn resolve_alert(
    github: &impl GitHub,
    context: &Context,
    run_url: &str,
) -> Result<(), AppError> {
    let identity = marker::issue(&context.instance, IssueKind::FailureAlert);
    let Some(issue) = find_issue(github, &context.repository, &identity).await? else {
        note(context, "no failure alert exists, so resolution is a no-op");
        return Ok(());
    };
    let body = message::resolved_failure_issue(&issue.body, run_url);
    github
        .update_issue(&context.repository, issue.number, None, &body)
        .await?;
    github.close_issue(&context.repository, issue.number).await
}

pub(crate) async fn pr_comment_preflight(
    github: &impl GitHub,
    context: &Context,
    pull_request: u64,
    packages: &str,
    head: &CommitSha,
    run_id: u64,
) -> Result<(), AppError> {
    let identity = context.pr_identity();
    let existing = find_comment(github, &context.repository, pull_request, &identity).await?;
    let live = github
        .pull_request_head(&context.repository, pull_request)
        .await?;
    if live != *head {
        note(
            context,
            "the PR advanced; not starting an obsolete run's comment lifecycle",
        );
        return Ok(());
    }
    match existing {
        None => {
            let body =
                message::pr_in_progress(&context.instance, &identity, packages, head, run_id);
            create_comment_reconciled(github, context, pull_request, &identity, &body).await
        }
        Some(comment)
            if message::is_in_progress(&comment.body, &context.instance)
                || message::is_terminal_note(&comment.body, &context.instance) =>
        {
            let body =
                message::pr_in_progress(&context.instance, &identity, packages, head, run_id);
            github
                .update_comment(&context.repository, comment.id, &body)
                .await
        }
        Some(comment) => {
            let comparison = match marker::find_analyzed_sha(&comment.body, &context.instance) {
                Some(analyzed) if analyzed != live => {
                    compare_or_unknown(github, context, &analyzed, &live).await
                }
                Some(_) => return Ok(()),
                None => Comparison { ahead_by: None },
            };
            let warning = message::stale_warning(comparison.ahead_by, "Benchmark results are");
            let body = message::insert_stale_banner(&comment.body, &context.instance, &warning);
            github
                .update_comment(&context.repository, comment.id, &body)
                .await
        }
    }
}

pub(crate) async fn publish_pr_comment(
    github: &impl GitHub,
    context: &Context,
    pull_request: u64,
    evidence: &Evidence,
    packages: &str,
    summary: &str,
    envelope: Envelope<'_>,
) -> Result<(), AppError> {
    evidence.report.require_mode(AnalysisMode::Branch)?;
    let analyzed_sha = &evidence.report.commit;
    let identity = context.pr_identity();
    let existing = find_comment(github, &context.repository, pull_request, &identity).await?;
    let mut body = message::pr_result(
        &context.instance,
        &identity,
        evidence,
        packages,
        summary,
        envelope,
    );
    match github
        .pull_request_head(&context.repository, pull_request)
        .await
    {
        Ok(live) if live != *analyzed_sha => {
            if existing.as_ref().is_some_and(|comment| {
                marker::find_analyzed_sha(&comment.body, &context.instance).as_ref() == Some(&live)
            }) {
                note(
                    context,
                    "the rolling comment already describes the live head; preserving it",
                );
                return Ok(());
            }
            let comparison = compare_or_unknown(github, context, analyzed_sha, &live).await;
            let warning = message::stale_warning(comparison.ahead_by, "Benchmark results are");
            body = message::insert_stale_banner(&body, &context.instance, &warning);
        }
        Ok(_) => {}
        Err(error) => {
            eprintln!("Could not verify report freshness: {error}");
            let warning = message::freshness_unverified("Benchmark result");
            body = message::insert_stale_banner(&body, &context.instance, &warning);
        }
    }

    match existing {
        Some(comment) => {
            github
                .update_comment(&context.repository, comment.id, &body)
                .await
        }
        None => create_comment_reconciled(github, context, pull_request, &identity, &body).await,
    }
}

async fn compare_or_unknown(
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

pub(crate) async fn pr_comment_cleanup(
    github: &impl GitHub,
    context: &Context,
    pull_request: u64,
    head: &CommitSha,
    delete: bool,
) -> Result<(), AppError> {
    let identity = context.pr_identity();
    let existing = find_comment(github, &context.repository, pull_request, &identity).await?;
    let live = github
        .pull_request_head(&context.repository, pull_request)
        .await?;
    if live != *head {
        note(
            context,
            "the PR advanced; not clearing a newer run's comment",
        );
        return Ok(());
    }
    let body = message::pr_nothing_in_scope(&context.instance, &identity);
    match (existing, delete) {
        (Some(comment), true) => github.delete_comment(&context.repository, comment.id).await,
        (Some(comment), false) => {
            github
                .update_comment(&context.repository, comment.id, &body)
                .await
        }
        (None, false) => {
            create_comment_reconciled(github, context, pull_request, &identity, &body).await
        }
        (None, true) => Ok(()),
    }
}

pub(crate) async fn pr_comment_finalize(
    github: &impl GitHub,
    context: &Context,
    pull_request: u64,
    run_url: &str,
    head: &CommitSha,
    run_id: u64,
) -> Result<(), AppError> {
    let identity = context.pr_identity();
    let Some(comment) = find_comment(github, &context.repository, pull_request, &identity).await?
    else {
        return Ok(());
    };
    if !message::is_in_progress(&comment.body, &context.instance) {
        note(
            context,
            "the rolling comment already carries results, so finalize is a no-op",
        );
        return Ok(());
    }
    let owner = marker::run_owner(&context.instance, run_id, head);
    if !comment.body.lines().any(|line| line == owner) {
        note(
            context,
            "the placeholder belongs to a different run; leaving it unchanged",
        );
        return Ok(());
    }
    let body = message::pr_failed(&context.instance, &identity, run_url);
    github
        .update_comment(&context.repository, comment.id, &body)
        .await
}

async fn upsert_issue(
    github: &impl GitHub,
    context: &Context,
    title: &str,
    marker: &str,
    body: &str,
    commit: Option<&CommitSha>,
) -> Result<(), AppError> {
    if let Some(issue) = find_issue(github, &context.repository, marker).await? {
        if let Some(commit) = commit
            && !may_replace_issue(github, context, &issue, commit).await
        {
            return Ok(());
        }
        return github
            .update_issue(&context.repository, issue.number, Some(title), body)
            .await;
    }
    match github.create_issue(&context.repository, title, body).await {
        Ok(_) => Ok(()),
        Err(error) => reconcile_create(
            error,
            find_issue(github, &context.repository, marker)
                .await
                .map(|issue| issue.map(|issue| issue.body)),
            body,
        ),
    }
}

async fn may_replace_issue(
    github: &impl GitHub,
    context: &Context,
    issue: &Issue,
    commit: &CommitSha,
) -> bool {
    let Some(existing) = marker::find_analyzed_sha(&issue.body, &context.instance) else {
        eprintln!("Preserving regression issue: its analyzed commit could not be verified.");
        return false;
    };
    if existing == *commit {
        return true;
    }
    let comparison = compare_or_unknown(github, context, &existing, commit).await;
    if comparison.ahead_by.is_some_and(|distance| distance > 0) {
        return true;
    }
    eprintln!("Preserving regression issue: the new report is not verified as newer.");
    false
}

async fn create_comment_reconciled(
    github: &impl GitHub,
    context: &Context,
    pull_request: u64,
    marker: &str,
    body: &str,
) -> Result<(), AppError> {
    match github
        .create_comment(&context.repository, pull_request, body)
        .await
    {
        Ok(_) => Ok(()),
        Err(error) => reconcile_create(
            error,
            find_comment(github, &context.repository, pull_request, marker)
                .await
                .map(|comment| comment.map(|comment| comment.body)),
            body,
        ),
    }
}

fn reconcile_create(
    error: AppError,
    observed: Result<Option<String>, AppError>,
    desired: &str,
) -> Result<(), AppError> {
    match observed {
        Ok(Some(body)) if body == desired => Ok(()),
        Ok(Some(_)) => Err(error.enrich(
            "marker reconciliation found different content; preserving the other publication",
        )),
        Ok(None) => Err(error),
        Err(_lookup_error) => {
            Err(error.enrich("marker reconciliation also failed after the create error"))
        }
    }
}

async fn find_issue(
    github: &impl GitHub,
    repository: &Repository,
    marker: &str,
) -> Result<Option<Issue>, AppError> {
    Ok(github
        .open_issues(repository)
        .await?
        .into_iter()
        .find(|issue| issue.body.lines().any(|line| line == marker)))
}

async fn find_comment(
    github: &impl GitHub,
    repository: &Repository,
    pull_request: u64,
    marker: &str,
) -> Result<Option<Comment>, AppError> {
    Ok(github
        .comments(repository, pull_request)
        .await?
        .into_iter()
        .find(|comment| comment.body.lines().any(|line| line == marker)))
}

// Verbose diagnostics have no behavioral effect, and capturing process stderr would
// add global-state coupling to otherwise hermetic unit tests.
#[cfg_attr(test, mutants::skip)]
fn note(context: &Context, message: &str) {
    if context.verbose {
        eprintln!(
            "[cargo-bench-history-github] {}: {message}",
            context.repository
        );
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use futures::executor::block_on;

    use super::*;
    use crate::errors::AmbiguousCreateError;
    use crate::github::fake::FakeGitHub;
    use crate::result::tests::evidence;
    use crate::result::{Outcome, UnsafeAllClear};

    fn context() -> Context {
        Context {
            repository: "folo-rs/folo".parse().unwrap(),
            instance: "default".parse().unwrap(),
            verbose: false,
            comment_marker: None,
        }
    }

    fn sha(value: char) -> CommitSha {
        value.to_string().repeat(40).parse().unwrap()
    }

    fn evidence_at(mode: AnalysisMode, outcome: Outcome, commit: char) -> Evidence {
        let mut evidence = evidence(mode, outcome, true);
        evidence.report.commit = sha(commit);
        evidence
    }

    fn only_issue(github: &FakeGitHub) -> Issue {
        let issues = github.issues();
        assert_eq!(issues.len(), 1);
        issues.into_iter().next().unwrap()
    }

    fn only_comment(github: &FakeGitHub, pull_request: u64) -> Comment {
        let comments = github.comments_for(pull_request);
        assert_eq!(comments.len(), 1);
        comments.into_iter().next().unwrap()
    }

    #[test]
    fn ambiguous_issue_create_is_reconciled_by_marker() {
        let github = FakeGitHub::new();
        github.fail_next_issue_create_after_commit();
        block_on(publish_issue(
            &github,
            &context(),
            "Regressions",
            "summary",
            &evidence_at(AnalysisMode::History, Outcome::Findings, 'a'),
            Envelope::default(),
        ))
        .unwrap();
        assert_eq!(github.issues().len(), 1);
    }

    #[test]
    fn create_error_survives_a_failed_issue_reconciliation() {
        let github = FakeGitHub::new();
        github.fail_next_issue_create_after_commit();
        github.fail_issue_list();
        let error = block_on(publish_issue(
            &github,
            &context(),
            "Regressions",
            "summary",
            &evidence_at(AnalysisMode::History, Outcome::Findings, 'a'),
            Envelope::default(),
        ))
        .unwrap_err();
        assert!(error.find_source::<AmbiguousCreateError>().is_some());
    }

    #[test]
    fn publishing_again_updates_the_displayed_issue_title() {
        let github = FakeGitHub::new();
        let context = context();
        github.set_comparison(&sha('a'), &sha('b'), Comparison { ahead_by: Some(1) });
        block_on(publish_issue(
            &github,
            &context,
            "Old title",
            "summary",
            &evidence_at(AnalysisMode::History, Outcome::Findings, 'a'),
            Envelope::default(),
        ))
        .unwrap();
        block_on(publish_issue(
            &github,
            &context,
            "New title",
            "summary",
            &evidence_at(AnalysisMode::History, Outcome::Findings, 'b'),
            Envelope::default(),
        ))
        .unwrap();

        assert_eq!(only_issue(&github).title, "New title");
    }

    #[test]
    fn issue_preflight_replaces_staleness_banner() {
        let github = FakeGitHub::new();
        let context = context();
        block_on(publish_issue(
            &github,
            &context,
            "Regressions",
            "summary",
            &evidence_at(AnalysisMode::History, Outcome::Findings, 'a'),
            Envelope::default(),
        ))
        .unwrap();
        github.set_comparison(&sha('a'), &sha('b'), Comparison { ahead_by: Some(2) });
        block_on(issue_preflight(&github, &context, &sha('b'))).unwrap();
        block_on(issue_preflight(&github, &context, &sha('b'))).unwrap();

        let body = only_issue(&github).body;
        assert_eq!(body.matches("2 commits behind HEAD").count(), 1);
    }

    #[test]
    fn issue_cleanup_updates_before_optional_close() {
        let github = FakeGitHub::new();
        let context = context();
        github.set_comparison(&sha('a'), &sha('b'), Comparison { ahead_by: Some(1) });
        github.set_comparison(&sha('b'), &sha('c'), Comparison { ahead_by: Some(1) });
        block_on(publish_issue(
            &github,
            &context,
            "Regressions",
            "summary",
            &evidence_at(AnalysisMode::History, Outcome::Findings, 'a'),
            Envelope::default(),
        ))
        .unwrap();
        block_on(issue_cleanup(
            &github,
            &context,
            &evidence_at(AnalysisMode::History, Outcome::Clean, 'b'),
            false,
            Envelope::default(),
        ))
        .unwrap();
        assert!(
            only_issue(&github)
                .body
                .contains("No notable benchmark changes")
        );

        block_on(issue_cleanup(
            &github,
            &context,
            &evidence_at(AnalysisMode::History, Outcome::Clean, 'c'),
            true,
            Envelope::default(),
        ))
        .unwrap();
        assert!(github.issues().is_empty());
    }

    #[test]
    fn failure_alert_is_independent_of_regression_issue() {
        let github = FakeGitHub::new();
        let context = context();
        block_on(publish_issue(
            &github,
            &context,
            "Regressions",
            "summary",
            &evidence_at(AnalysisMode::History, Outcome::Findings, 'a'),
            Envelope::default(),
        ))
        .unwrap();
        block_on(alert(
            &github,
            &context,
            "Failure",
            "https://example.test/run",
            Envelope::default(),
        ))
        .unwrap();
        assert_eq!(github.issues().len(), 2);
        block_on(resolve_alert(
            &github,
            &context,
            "https://example.test/success",
        ))
        .unwrap();
        assert_eq!(github.issues().len(), 1);
    }

    #[test]
    fn pull_request_lifecycle_updates_one_comment() {
        let github = FakeGitHub::new();
        let context = context();
        let pull_request = 7;
        github.set_pull_head(pull_request, sha('a'));
        github.set_pull_head(pull_request, sha('a'));

        block_on(pr_comment_preflight(
            &github,
            &context,
            pull_request,
            "foo,bar",
            &sha('a'),
            1,
        ))
        .unwrap();
        assert!(message::is_in_progress(
            &only_comment(&github, pull_request).body,
            &context.instance
        ));

        block_on(publish_pr_comment(
            &github,
            &context,
            pull_request,
            &evidence_at(AnalysisMode::Branch, Outcome::Findings, 'a'),
            "foo,bar",
            "summary",
            Envelope::default(),
        ))
        .unwrap();
        assert!(only_comment(&github, pull_request).body.contains("summary"));

        block_on(pr_comment_cleanup(
            &github,
            &context,
            pull_request,
            &sha('a'),
            false,
        ))
        .unwrap();
        assert!(
            only_comment(&github, pull_request)
                .body
                .contains("No benchmarkable package")
        );
    }

    #[test]
    fn preflight_marks_existing_results_stale_but_not_current_results() {
        let github = FakeGitHub::new();
        let context = context();
        let pull_request = 13;
        github.set_pull_head(pull_request, sha('a'));
        github.set_pull_head(pull_request, sha('a'));
        block_on(publish_pr_comment(
            &github,
            &context,
            pull_request,
            &evidence_at(AnalysisMode::Branch, Outcome::Findings, 'a'),
            "foo",
            "summary",
            Envelope::default(),
        ))
        .unwrap();
        block_on(pr_comment_preflight(
            &github,
            &context,
            pull_request,
            "foo",
            &sha('a'),
            1,
        ))
        .unwrap();
        assert!(
            !only_comment(&github, pull_request)
                .body
                .contains("[!WARNING]")
        );

        github.set_pull_head(pull_request, sha('b'));
        github.set_comparison(&sha('a'), &sha('b'), Comparison { ahead_by: Some(1) });
        block_on(pr_comment_preflight(
            &github,
            &context,
            pull_request,
            "foo",
            &sha('b'),
            1,
        ))
        .unwrap();
        assert!(
            only_comment(&github, pull_request)
                .body
                .contains("1 commit behind HEAD")
        );
    }

    #[test]
    fn preflight_refreshes_an_existing_placeholder_scope() {
        let github = FakeGitHub::new();
        let context = context();
        let pull_request = 15;
        github.set_pull_head(pull_request, sha('a'));
        block_on(pr_comment_preflight(
            &github,
            &context,
            pull_request,
            "old-package",
            &sha('a'),
            1,
        ))
        .unwrap();
        block_on(pr_comment_preflight(
            &github,
            &context,
            pull_request,
            "new-package",
            &sha('a'),
            1,
        ))
        .unwrap();

        let body = only_comment(&github, pull_request).body;
        assert!(body.contains("`new-package`"));
        assert!(!body.contains("`old-package`"));
        assert!(!body.contains("[!WARNING]"));
    }

    #[test]
    fn marker_lookup_ignores_an_unrelated_comment() {
        let github = FakeGitHub::new();
        let context = context();
        let pull_request = 14;
        github.set_pull_head(pull_request, sha('a'));
        block_on(github.create_comment(&context.repository, pull_request, "unrelated")).unwrap();
        block_on(pr_comment_preflight(
            &github,
            &context,
            pull_request,
            "foo",
            &sha('a'),
            1,
        ))
        .unwrap();

        let comments = github.comments_for(pull_request);
        assert_eq!(comments.len(), 2);
        assert!(comments.iter().any(|one| one.body == "unrelated"));
        assert!(
            comments
                .iter()
                .any(|one| message::is_in_progress(&one.body, &context.instance))
        );
    }

    #[test]
    fn publish_marks_results_stale_when_head_advanced() {
        let github = FakeGitHub::new();
        let context = context();
        let pull_request = 8;
        github.set_pull_head(pull_request, sha('a'));
        github.set_pull_head(pull_request, sha('b'));
        github.set_comparison(&sha('a'), &sha('b'), Comparison { ahead_by: Some(1) });

        block_on(publish_pr_comment(
            &github,
            &context,
            pull_request,
            &evidence_at(AnalysisMode::Branch, Outcome::Findings, 'a'),
            "foo",
            "summary",
            Envelope::default(),
        ))
        .unwrap();

        let body = only_comment(&github, pull_request).body;
        assert!(body.contains("1 commit behind HEAD"));
    }

    #[test]
    fn publish_warns_when_freshness_cannot_be_verified() {
        let github = FakeGitHub::new();
        let context = context();
        let pull_request = 11;
        github.set_pull_head(pull_request, sha('a'));
        github.fail_pull_head();

        block_on(publish_pr_comment(
            &github,
            &context,
            pull_request,
            &evidence_at(AnalysisMode::Branch, Outcome::Findings, 'a'),
            "foo",
            "summary",
            Envelope::default(),
        ))
        .unwrap();

        let body = only_comment(&github, pull_request).body;
        assert!(body.contains("freshness could not be verified"));
    }

    #[test]
    fn ambiguous_comment_create_is_reconciled_by_marker() {
        let github = FakeGitHub::new();
        let context = context();
        let pull_request = 9;
        github.set_pull_head(pull_request, sha('a'));
        github.fail_next_comment_create_after_commit();

        block_on(publish_pr_comment(
            &github,
            &context,
            pull_request,
            &evidence_at(AnalysisMode::Branch, Outcome::Findings, 'a'),
            "foo",
            "summary",
            Envelope::default(),
        ))
        .unwrap();
        assert_eq!(github.comments_for(pull_request).len(), 1);
    }

    #[test]
    fn create_error_survives_a_failed_comment_reconciliation() {
        let github = FakeGitHub::new();
        let context = context();
        let pull_request = 16;
        github.set_pull_head(pull_request, sha('a'));
        github.fail_next_comment_create_after_commit();
        github.fail_comment_list();
        let error = block_on(publish_pr_comment(
            &github,
            &context,
            pull_request,
            &evidence_at(AnalysisMode::Branch, Outcome::Findings, 'a'),
            "foo",
            "summary",
            Envelope::default(),
        ))
        .unwrap_err();
        assert!(error.find_source::<AmbiguousCreateError>().is_some());
    }

    #[test]
    fn finalize_only_replaces_an_in_progress_comment() {
        let github = FakeGitHub::new();
        let context = context();
        let pull_request = 10;
        github.set_pull_head(pull_request, sha('a'));
        block_on(pr_comment_preflight(
            &github,
            &context,
            pull_request,
            "foo",
            &sha('a'),
            1,
        ))
        .unwrap();
        block_on(pr_comment_finalize(
            &github,
            &context,
            pull_request,
            "https://example.test/run",
            &sha('a'),
            1,
        ))
        .unwrap();
        assert!(
            only_comment(&github, pull_request)
                .body
                .contains("did not complete successfully")
        );
    }

    #[test]
    fn cleanup_can_delete_instead_of_leaving_a_note() {
        let github = FakeGitHub::new();
        let context = context();
        let pull_request = 12;
        github.set_pull_head(pull_request, sha('a'));
        block_on(pr_comment_preflight(
            &github,
            &context,
            pull_request,
            "foo",
            &sha('a'),
            1,
        ))
        .unwrap();
        block_on(pr_comment_cleanup(
            &github,
            &context,
            pull_request,
            &sha('a'),
            true,
        ))
        .unwrap();
        assert!(github.comments_for(pull_request).is_empty());
    }

    #[test]
    fn reconciliation_does_not_confuse_another_publication_with_this_request() {
        let github = FakeGitHub::new();
        let context = context();
        let identity = context.pr_identity();
        let other_body = format!("{identity}\nAnother run's report");
        let other = block_on(github.create_comment(&context.repository, 1, &other_body)).unwrap();
        github.fail_next_comment_create_after_commit();
        let requested = format!("{identity}\nThis run's report");

        let error = block_on(create_comment_reconciled(
            &github, &context, 1, &identity, &requested,
        ))
        .unwrap_err();

        assert!(error.find_source::<AmbiguousCreateError>().is_some());
        assert_eq!(
            github
                .comments_for(1)
                .into_iter()
                .find(|comment| comment.id == other.id)
                .unwrap(),
            other,
        );
    }

    fn assert_cleanup_preserves_issue(outcome: Outcome) {
        for complete in [false, true] {
            if outcome == Outcome::Clean && complete {
                continue;
            }
            let github = FakeGitHub::new();
            let context = context();
            block_on(publish_issue(
                &github,
                &context,
                "Regression",
                "existing finding",
                &evidence_at(AnalysisMode::History, Outcome::Findings, 'a'),
                Envelope::default(),
            ))
            .unwrap();
            let before = only_issue(&github);
            let incomplete = evidence(AnalysisMode::History, outcome, complete);
            let error = block_on(issue_cleanup(
                &github,
                &context,
                &incomplete,
                true,
                Envelope::default(),
            ))
            .unwrap_err();
            assert!(error.find_source::<UnsafeAllClear>().is_some());
            assert_eq!(only_issue(&github), before);
        }
    }

    #[test]
    fn issue_cleanup_never_clears_findings() {
        assert_cleanup_preserves_issue(Outcome::Findings);
    }

    #[test]
    fn issue_cleanup_never_clears_incomplete_collection() {
        assert_cleanup_preserves_issue(Outcome::Clean);
    }

    #[test]
    fn issue_cleanup_never_clears_insufficient_baselines() {
        assert_cleanup_preserves_issue(Outcome::InsufficientBaseline);
    }

    #[test]
    fn issue_cleanup_never_clears_an_empty_scope() {
        assert_cleanup_preserves_issue(Outcome::NothingInScope);
    }

    #[test]
    fn issue_cleanup_never_clears_partial_analysis() {
        assert_cleanup_preserves_issue(Outcome::Partial);
    }

    #[test]
    fn partial_findings_are_disclosed_in_both_sinks() {
        let github = FakeGitHub::new();
        let context = context();
        github.set_pull_head(1, sha('a'));
        let history = evidence(AnalysisMode::History, Outcome::Findings, false);
        block_on(publish_issue(
            &github,
            &context,
            "Regressions",
            "tool finding",
            &history,
            Envelope::default(),
        ))
        .unwrap();
        let branch = evidence(AnalysisMode::Branch, Outcome::Findings, false);
        block_on(publish_pr_comment(
            &github,
            &context,
            1,
            &branch,
            "package",
            "tool finding",
            Envelope::default(),
        ))
        .unwrap();
        for body in [only_issue(&github).body, only_comment(&github, 1).body] {
            assert!(body.contains("Notable benchmark changes detected."));
            assert!(body.contains("Partial platform coverage."));
            assert!(body.contains("Completed: linux."));
            assert!(body.contains("Missing: windows."));
            assert!(body.contains("tool finding"));
        }
    }

    #[test]
    fn empty_scope_creates_a_note_without_a_previous_comment() {
        let github = FakeGitHub::new();
        let context = context();
        github.set_pull_head(1, sha('a'));
        block_on(pr_comment_cleanup(&github, &context, 1, &sha('a'), false)).unwrap();
        let first = only_comment(&github, 1);
        assert!(first.body.contains("No benchmarkable package"));
        block_on(pr_comment_cleanup(&github, &context, 1, &sha('a'), false)).unwrap();
        assert_eq!(only_comment(&github, 1), first);
    }

    #[test]
    fn a_new_run_refreshes_empty_and_failed_notes_into_owned_placeholders() {
        for failed in [false, true] {
            let github = FakeGitHub::new();
            let context = context();
            github.set_pull_head(1, sha('a'));
            if failed {
                block_on(pr_comment_preflight(
                    &github,
                    &context,
                    1,
                    "old",
                    &sha('a'),
                    1,
                ))
                .unwrap();
                block_on(pr_comment_finalize(
                    &github,
                    &context,
                    1,
                    "https://example.test/run",
                    &sha('a'),
                    1,
                ))
                .unwrap();
            } else {
                block_on(pr_comment_cleanup(&github, &context, 1, &sha('a'), false)).unwrap();
            }
            block_on(pr_comment_preflight(
                &github,
                &context,
                1,
                "new",
                &sha('a'),
                2,
            ))
            .unwrap();
            let body = only_comment(&github, 1).body;
            assert!(message::is_in_progress(&body, &context.instance));
            assert!(body.contains(&marker::run_owner(&context.instance, 2, &sha('a'))));
            assert!(body.contains("`new`"));
            assert!(!body.contains("[!WARNING]"));
        }
    }

    #[test]
    fn late_finalizers_and_empty_scope_runs_preserve_newer_results() {
        let github = FakeGitHub::new();
        let context = context();
        github.set_pull_head(1, sha('a'));
        block_on(pr_comment_preflight(
            &github,
            &context,
            1,
            "foo",
            &sha('a'),
            2,
        ))
        .unwrap();
        let before = only_comment(&github, 1);
        block_on(pr_comment_finalize(
            &github,
            &context,
            1,
            "https://example.test/old",
            &sha('a'),
            1,
        ))
        .unwrap();
        assert_eq!(only_comment(&github, 1), before);
        block_on(pr_comment_finalize(
            &github,
            &context,
            1,
            "https://example.test/old",
            &sha('b'),
            2,
        ))
        .unwrap();
        assert_eq!(only_comment(&github, 1), before);
        github.set_pull_head(1, sha('b'));
        block_on(pr_comment_cleanup(&github, &context, 1, &sha('a'), false)).unwrap();
        block_on(pr_comment_cleanup(&github, &context, 1, &sha('a'), true)).unwrap();
        assert_eq!(only_comment(&github, 1), before);
    }

    #[test]
    fn old_publication_cannot_replace_results_for_the_live_head() {
        let github = FakeGitHub::new();
        let context = context();
        github.set_pull_head(1, sha('b'));
        let current = evidence_at(AnalysisMode::Branch, Outcome::Findings, 'b');
        block_on(publish_pr_comment(
            &github,
            &context,
            1,
            &current,
            "foo",
            "current summary",
            Envelope::default(),
        ))
        .unwrap();
        let before = only_comment(&github, 1);
        let old = evidence_at(AnalysisMode::Branch, Outcome::Findings, 'a');
        block_on(publish_pr_comment(
            &github,
            &context,
            1,
            &old,
            "foo",
            "obsolete summary",
            Envelope::default(),
        ))
        .unwrap();
        assert_eq!(only_comment(&github, 1), before);
    }

    #[test]
    fn publication_rejects_wrong_modes_and_nonfinding_issues() {
        let github = FakeGitHub::new();
        let context = context();
        let clean = evidence_at(AnalysisMode::History, Outcome::Clean, 'a');
        block_on(publish_issue(
            &github,
            &context,
            "Regression",
            "summary",
            &clean,
            Envelope::default(),
        ))
        .unwrap_err();
        let branch = evidence_at(AnalysisMode::Branch, Outcome::Findings, 'a');
        block_on(publish_issue(
            &github,
            &context,
            "Regression",
            "summary",
            &branch,
            Envelope::default(),
        ))
        .unwrap_err();
        block_on(publish_pr_comment(
            &github,
            &context,
            1,
            &clean,
            "foo",
            "summary",
            Envelope::default(),
        ))
        .unwrap_err();
        assert!(github.issues().is_empty());
        assert!(github.comments_for(1).is_empty());
    }

    #[test]
    fn custom_comment_identity_is_used_across_the_lifecycle() {
        let github = FakeGitHub::new();
        let mut context = context();
        context.comment_marker = Some("<!-- custom-performance -->".parse().unwrap());
        github.set_pull_head(1, sha('a'));
        block_on(pr_comment_preflight(
            &github,
            &context,
            1,
            "foo",
            &sha('a'),
            1,
        ))
        .unwrap();
        let id = only_comment(&github, 1).id;
        block_on(publish_pr_comment(
            &github,
            &context,
            1,
            &evidence_at(AnalysisMode::Branch, Outcome::Findings, 'a'),
            "foo",
            "summary",
            Envelope::default(),
        ))
        .unwrap();
        block_on(pr_comment_cleanup(&github, &context, 1, &sha('a'), false)).unwrap();
        let comment = only_comment(&github, 1);
        assert_eq!(comment.id, id);
        assert!(comment.body.starts_with("<!-- custom-performance -->"));
        assert!(!comment.body.contains(":pr-comment"));
    }

    #[test]
    fn delayed_history_reports_cannot_clear_or_replace_newer_findings() {
        let github = FakeGitHub::new();
        let context = context();
        let latest = evidence_at(AnalysisMode::History, Outcome::Findings, 'b');
        block_on(publish_issue(
            &github,
            &context,
            "Regression",
            "latest findings",
            &latest,
            Envelope::default(),
        ))
        .unwrap();
        let before = only_issue(&github);
        let older = evidence_at(AnalysisMode::History, Outcome::Clean, 'a');
        block_on(issue_cleanup(
            &github,
            &context,
            &older,
            true,
            Envelope::default(),
        ))
        .unwrap();
        assert_eq!(only_issue(&github), before);
        let older = evidence_at(AnalysisMode::History, Outcome::Findings, 'a');
        block_on(publish_issue(
            &github,
            &context,
            "Old title",
            "old findings",
            &older,
            Envelope::default(),
        ))
        .unwrap();
        assert_eq!(only_issue(&github), before);
    }

    #[test]
    fn a_zero_distance_does_not_establish_forward_issue_progress() {
        let github = FakeGitHub::new();
        let context = context();
        let issue = Issue {
            number: 1,
            title: "Regression".to_owned(),
            body: marker::analyzed_sha(&context.instance, &sha('a')),
        };
        github.set_comparison(&sha('a'), &sha('b'), Comparison { ahead_by: Some(0) });
        assert!(!block_on(may_replace_issue(
            &github,
            &context,
            &issue,
            &sha('b')
        )));
        assert!(block_on(may_replace_issue(
            &github,
            &context,
            &issue,
            &sha('a')
        )));
    }
}
