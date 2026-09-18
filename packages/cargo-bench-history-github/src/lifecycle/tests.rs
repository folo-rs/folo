use std::num::NonZero;
use std::time::{Duration, SystemTime};

use futures::executor::block_on;
use tick::Clock;

use crate::cli::{Conclusion, FailedArgs, PendingArgs, RunArgs};
use crate::errors::AmbiguousCreateError;
use crate::github::fake::FakeGitHub;
use crate::github::{Comment, Comparison, GitHub, Issue};
use crate::identity::IssueIdentity;
use crate::lifecycle::*;
use crate::model::{CommitSha, IssueKind};
use crate::operations::Context;
use crate::result::tests::evidence;
use crate::result::{AnalysisMode, Coverage, Outcome, PublicationState, WrongPublicationState};
use crate::{marker, message};

fn context() -> Context {
    Context {
        repository: "folo-rs/folo".parse().unwrap(),
        instance: "project".parse().unwrap(),
        verbose: false,
    }
}

fn clock(day: u64) -> Clock {
    // Days relative to the epoch make update/no-op tests independent of calendar formatting.
    Clock::new_frozen_at(
        SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(day.checked_mul(86400).unwrap()))
            .unwrap(),
    )
}

fn sha(value: char) -> CommitSha {
    value.to_string().repeat(40).parse().unwrap()
}

fn owner(run: u64, attempt: u64, head: char) -> PendingArgs {
    PendingArgs {
        run: RunArgs {
            run_id: NonZero::new(run).unwrap(),
            run_attempt: NonZero::new(attempt).unwrap(),
        },
        head: sha(head),
    }
}

fn report(mode: AnalysisMode, outcome: Outcome, complete: bool, owner: PendingArgs) -> Report {
    let mut evidence = evidence(mode, outcome, complete);
    evidence.report.commit = owner.head.clone();
    Report {
        owner,
        evidence,
        summary: "  Tool-rendered summary.\n\n".to_owned(),
        artifact_url: Some("https://example.test/artifact".to_owned()),
    }
}

fn findings(head: char) -> Report {
    report(
        AnalysisMode::History,
        Outcome::Findings,
        true,
        owner(1, 1, head),
    )
}

fn failure(owner: PendingArgs, conclusion: Conclusion) -> FailedArgs {
    FailedArgs {
        run_url: format!(
            "https://github.com/folo-rs/folo/actions/runs/{}",
            owner.run.run_id
        ),
        pending: owner,
        conclusion,
    }
}

fn only_issue(github: &FakeGitHub) -> Issue {
    let issues = github.issues();
    assert_eq!(issues.len(), 1);
    issues.into_iter().next().unwrap()
}

fn only_comment(github: &FakeGitHub) -> Comment {
    let comments = github.comments_for(7);
    assert_eq!(comments.len(), 1);
    comments.into_iter().next().unwrap()
}

fn assert_comment_metadata_rejected(body: &str) {
    let github = FakeGitHub::new();
    let context = context();
    github.set_pull_head(7, sha('a'));
    let before = block_on(github.create_comment(&context.repository, 7, body)).unwrap();
    block_on(comment_preflight(
        &github,
        &context,
        7,
        "foo",
        &owner(2, 1, 'a'),
    ))
    .unwrap_err();
    assert_eq!(only_comment(&github), before);
}

fn publish(github: &FakeGitHub, report: &Report, day: u64) {
    block_on(issue_report(
        github,
        &context(),
        &clock(day),
        report,
        report.evidence.publication_state(),
    ))
    .unwrap();
}

#[test]
fn only_findings_create_issues_and_explicit_states_cannot_misrepresent_evidence() {
    let github = FakeGitHub::new();
    let clean = report(
        AnalysisMode::History,
        Outcome::Clean,
        true,
        owner(1, 1, 'a'),
    );
    block_on(issue_report(
        &github,
        &context(),
        &clock(1),
        &clean,
        PublicationState::Clean,
    ))
    .unwrap();
    block_on(issue_preflight(
        &github,
        &context(),
        &clock(1),
        &owner(1, 1, 'a'),
    ))
    .unwrap();
    block_on(issue_no_data(
        &github,
        &context(),
        &clock(1),
        &NoData::Empty(owner(1, 1, 'a')),
    ))
    .unwrap();
    block_on(issue_failed(
        &github,
        &context(),
        &clock(1),
        &failure(owner(1, 1, 'a'), Conclusion::Failure),
    ))
    .unwrap();
    assert!(github.issues().is_empty());
    for (outcome, complete, state) in [
        (Outcome::Clean, true, PublicationState::Findings),
        (Outcome::Findings, true, PublicationState::Clean),
        (Outcome::Clean, false, PublicationState::Clean),
        (Outcome::Partial, true, PublicationState::Clean),
    ] {
        let report = report(AnalysisMode::History, outcome, complete, owner(1, 1, 'a'));
        let error =
            block_on(issue_report(&github, &context(), &clock(1), &report, state)).unwrap_err();
        assert!(error.find_source::<WrongPublicationState>().is_some());
    }
    let error = block_on(issue_no_data(
        &github,
        &context(),
        &clock(1),
        &NoData::Report(findings('a')),
    ))
    .unwrap_err();
    assert!(error.find_source::<WrongPublicationState>().is_some());
    assert!(github.issues().is_empty());
}

#[test]
fn findings_remain_findings_with_partial_platform_and_series_coverage() {
    let github = FakeGitHub::new();
    let mut findings = report(
        AnalysisMode::History,
        Outcome::Findings,
        false,
        owner(1, 1, 'a'),
    );
    findings.evidence.report.coverage = Coverage::Partial;
    publish(&github, &findings, 1);
    let issue = only_issue(&github);
    assert!(issue.body.contains("Notable benchmark changes detected."));
    assert!(issue.body.contains("Missing: windows."));
    assert!(
        issue
            .body
            .contains("Some in-scope metric series could not be judged.")
    );
    assert!(issue.body.contains(&findings.summary));
    assert!(issue.body.contains(findings.artifact_url.as_ref().unwrap()));
}

#[test]
fn clean_replaces_report_leaves_issue_open_and_date_follows_body_updates_only() {
    let github = FakeGitHub::new();
    publish(&github, &findings('a'), 1);
    let before = only_issue(&github);
    publish(&github, &findings('a'), 2);
    assert_eq!(only_issue(&github), before);
    let clean = report(
        AnalysisMode::History,
        Outcome::Clean,
        true,
        owner(2, 1, 'b'),
    );
    github.set_comparison(&sha('a'), &sha('b'), Comparison { ahead_by: Some(1) });
    publish(&github, &clean, 2);
    let after = only_issue(&github);
    assert_eq!(after.number, before.number);
    assert_ne!(after.title, before.title);
    assert!(after.open);
    assert!(after.body.contains("No notable changes detected"));
    assert_eq!(
        marker::find_analyzed_sha(&after.body, &context().instance),
        Some(sha('b'))
    );
    assert!(after.body.contains(&clean.summary));
    publish(&github, &clean, 3);
    assert_eq!(only_issue(&github), after);
}

#[test]
fn preflight_marks_clean_results_stale_without_changing_the_retained_verdict() {
    let github = FakeGitHub::new();
    let context = context();
    publish(&github, &findings('a'), 1);
    let clean = report(
        AnalysisMode::History,
        Outcome::Clean,
        true,
        owner(2, 1, 'a'),
    );
    publish(&github, &clean, 2);
    github.set_comparison(&sha('a'), &sha('b'), Comparison { ahead_by: Some(1) });
    let pending = owner(3, 1, 'b');
    block_on(issue_preflight(&github, &context, &clock(3), &pending)).unwrap();
    let issue = only_issue(&github);
    let parsed = IssueBody::parse(&issue.body, &context.instance).unwrap();
    assert_eq!(parsed.commit, sha('a'));
    assert_eq!(
        marker::find_state(parsed.report, &context.instance),
        Some("clean")
    );
    assert!(
        parsed
            .report
            .contains("Benchmark results are 1 commit behind HEAD.")
    );
    assert!(parsed.report.contains("No notable changes detected"));
    assert!(parsed.report.contains(&clean.summary));
    let annotation = parsed.annotation.unwrap();
    assert!(matches!(annotation.state, AnnotationState::Preflight));
    assert_eq!(annotation.owner, pending);
    assert!(issue.open);
}

#[test]
fn no_data_retains_report_commit_and_staleness_and_replaces_one_annotation() {
    let github = FakeGitHub::new();
    publish(&github, &findings('a'), 1);
    github.set_comparison(&sha('a'), &sha('b'), Comparison { ahead_by: Some(2) });
    let pending = owner(2, 1, 'b');
    block_on(issue_preflight(&github, &context(), &clock(2), &pending)).unwrap();
    let pending_body = only_issue(&github);
    let retained = IssueBody::parse(&pending_body.body, &context().instance)
        .unwrap()
        .report
        .to_owned();
    let partial = report(AnalysisMode::History, Outcome::Clean, false, pending);
    block_on(issue_no_data(
        &github,
        &context(),
        &clock(3),
        &NoData::Report(partial),
    ))
    .unwrap();
    let after = only_issue(&github);
    let parsed = IssueBody::parse(&after.body, &context().instance).unwrap();
    assert_eq!(parsed.report, retained);
    assert_eq!(parsed.commit, sha('a'));
    assert!(after.body.contains("2 commits behind HEAD"));
    assert!(after.body.contains("Missing: windows."));
    assert!(after.body.contains("completed platforms only"));
    assert_eq!(
        after
            .body
            .matches(&marker::annotation_start(&context().instance))
            .count(),
        1
    );
    assert_ne!(after.title, pending_body.title);
}

fn pending_issue() -> (FakeGitHub, PendingArgs, Issue) {
    let github = FakeGitHub::new();
    publish(&github, &findings('a'), 1);
    github.set_comparison(&sha('a'), &sha('b'), Comparison { ahead_by: Some(1) });
    let pending = owner(2, 2, 'b');
    block_on(issue_preflight(&github, &context(), &clock(2), &pending)).unwrap();
    let before = only_issue(&github);
    (github, pending, before)
}

fn assert_issue_rejects_obsolete_owner(obsolete: PendingArgs) {
    let (github, _, before) = pending_issue();
    block_on(issue_failed(
        &github,
        &context(),
        &clock(3),
        &failure(obsolete, Conclusion::Failure),
    ))
    .unwrap();
    assert_eq!(only_issue(&github), before);
}

#[test]
fn failed_issue_cannot_retire_another_run() {
    assert_issue_rejects_obsolete_owner(owner(1, 2, 'b'));
}

#[test]
fn failed_issue_cannot_retire_another_attempt() {
    assert_issue_rejects_obsolete_owner(owner(2, 1, 'b'));
}

#[test]
fn failed_issue_cannot_retire_another_head() {
    assert_issue_rejects_obsolete_owner(owner(2, 2, 'c'));
}

#[test]
fn failed_issue_cancels_its_annotation_and_preserves_the_report() {
    let (github, pending, before) = pending_issue();
    block_on(issue_failed(
        &github,
        &context(),
        &clock(3),
        &failure(pending, Conclusion::Cancelled),
    ))
    .unwrap();
    let after = only_issue(&github);
    assert!(after.body.contains("was cancelled"));
    assert_eq!(
        IssueBody::parse(&after.body, &context().instance)
            .unwrap()
            .report,
        IssueBody::parse(&before.body, &context().instance)
            .unwrap()
            .report
    );
}

#[test]
fn failed_issue_does_not_replace_a_terminal_annotation() {
    let (github, pending, mut before) = pending_issue();
    let report = IssueBody::parse(&before.body, &context().instance)
        .unwrap()
        .report;
    before.body = annotate(
        report,
        &context().instance,
        &pending,
        "failed",
        "Terminal notice",
    );
    github.seed_issue(before.clone());
    block_on(issue_failed(
        &github,
        &context(),
        &clock(4),
        &failure(pending, Conclusion::Failure),
    ))
    .unwrap();
    assert_eq!(only_issue(&github), before);
}

#[test]
fn partial_publication_is_not_overwritten_by_failure_or_its_independent_alert() {
    let github = FakeGitHub::new();
    let report = report(
        AnalysisMode::History,
        Outcome::Findings,
        false,
        owner(2, 1, 'a'),
    );
    publish(&github, &report, 1);
    let before = only_issue(&github);
    block_on(issue_failed(
        &github,
        &context(),
        &clock(2),
        &failure(report.owner.clone(), Conclusion::Failure),
    ))
    .unwrap();
    assert_eq!(only_issue(&github), before);
    block_on(alert(
        &github,
        &context(),
        report.owner.run.run_id,
        &failure(report.owner, Conclusion::Failure).run_url,
    ))
    .unwrap();
    assert_eq!(github.issues().len(), 2);
    assert!(github.issues().contains(&before));
}

#[test]
fn older_or_unorderable_reports_do_not_replace_findings_or_annotations() {
    let github = FakeGitHub::new();
    publish(&github, &findings('b'), 1);
    let before = only_issue(&github);
    for incoming in [
        report(
            AnalysisMode::History,
            Outcome::Findings,
            true,
            owner(2, 1, 'a'),
        ),
        report(
            AnalysisMode::History,
            Outcome::Clean,
            true,
            owner(2, 1, 'a'),
        ),
    ] {
        publish(&github, &incoming, 2);
        assert_eq!(only_issue(&github), before);
    }
    github.set_comparison(&sha('a'), &sha('b'), Comparison { ahead_by: Some(1) });
    block_on(issue_preflight(
        &github,
        &context(),
        &clock(2),
        &owner(2, 1, 'a'),
    ))
    .unwrap();
    assert_eq!(only_issue(&github), before);
    block_on(issue_no_data(
        &github,
        &context(),
        &clock(2),
        &NoData::Empty(owner(2, 1, 'a')),
    ))
    .unwrap();
    assert_eq!(only_issue(&github), before);
}

#[test]
fn unknown_preflight_distance_is_visible_and_reruns_protect_newer_annotations() {
    let github = FakeGitHub::new();
    publish(&github, &findings('a'), 1);
    block_on(issue_preflight(
        &github,
        &context(),
        &clock(2),
        &owner(2, 2, 'b'),
    ))
    .unwrap();
    let before = only_issue(&github);
    assert!(before.body.contains("distance is unavailable"));
    block_on(issue_preflight(
        &github,
        &context(),
        &clock(3),
        &owner(2, 1, 'b'),
    ))
    .unwrap();
    assert_eq!(only_issue(&github), before);
    publish(
        &github,
        &report(
            AnalysisMode::History,
            Outcome::Findings,
            true,
            owner(2, 1, 'b'),
        ),
        3,
    );
    assert_eq!(only_issue(&github), before);
}

#[test]
fn similar_projects_are_not_adopted_and_duplicates_or_invalid_bodies_are_errors() {
    let github = FakeGitHub::new();
    let mut other = context();
    other.instance = "project.extra".parse().unwrap();
    block_on(issue_report(
        &github,
        &other,
        &clock(1),
        &findings('a'),
        PublicationState::Findings,
    ))
    .unwrap();
    publish(&github, &findings('a'), 1);
    assert_eq!(github.issues().len(), 2);
    let issue = github
        .issues()
        .into_iter()
        .find(|issue| IssueIdentity::Rolling(context().instance).matches(&issue.title))
        .unwrap();
    let mut duplicate = issue.clone();
    duplicate.number = 99;
    github.seed_issue(duplicate);
    block_on(issue_preflight(
        &github,
        &context(),
        &clock(1),
        &owner(2, 1, 'b'),
    ))
    .unwrap_err();
    let github = FakeGitHub::new();
    for body in [
        "human body".to_owned(),
        format!(
            "{}\n{}",
            issue.body,
            marker::annotation_start(&context().instance)
        ),
        format!(
            "{}\n{}",
            issue.body,
            marker::analyzed_sha(&context().instance, &sha('a'))
        ),
    ] {
        let mut invalid = issue.clone();
        invalid.body = body;
        github.seed_issue(invalid.clone());
        block_on(issue_preflight(
            &github,
            &context(),
            &clock(2),
            &owner(2, 1, 'b'),
        ))
        .unwrap_err();
        assert_eq!(only_issue(&github), invalid);
    }
}

#[test]
fn alert_is_one_per_run_and_preserves_human_closed_content() {
    let github = FakeGitHub::new();
    let args = failure(owner(42, 1, 'a'), Conclusion::Failure);
    block_on(alert(
        &github,
        &context(),
        args.pending.run.run_id,
        &args.run_url,
    ))
    .unwrap();
    let issue = only_issue(&github);
    assert_eq!(
        issue.title,
        "Benchmark history workflow failed for project (run 42)"
    );
    github.human_close(issue.number);
    let before = only_issue(&github);
    block_on(alert(
        &github,
        &context(),
        args.pending.run.run_id,
        &args.run_url,
    ))
    .unwrap();
    assert_eq!(only_issue(&github), before);
    let other = failure(owner(43, 1, 'a'), Conclusion::Failure);
    block_on(alert(
        &github,
        &context(),
        other.pending.run.run_id,
        &other.run_url,
    ))
    .unwrap();
    assert_eq!(github.issues().len(), 2);
}

#[test]
fn invalid_run_links_fail_even_without_an_issue_or_placeholder() {
    let github = FakeGitHub::new();
    let mut args = failure(owner(42, 1, 'a'), Conclusion::Failure);
    args.run_url = "https://github.com/folo-rs/folo/actions/runs/43".to_owned();
    block_on(alert(
        &github,
        &context(),
        args.pending.run.run_id,
        &args.run_url,
    ))
    .unwrap_err();
    block_on(issue_failed(&github, &context(), &clock(1), &args)).unwrap_err();
    block_on(comment_failed(&github, &context(), 7, &args)).unwrap_err();
    assert!(github.issues().is_empty());
}

#[test]
fn ambiguous_issue_and_comment_creates_reconcile_without_another_create() {
    let github = FakeGitHub::new();
    github.fail_next_issue_create_after_commit();
    publish(&github, &findings('a'), 1);
    assert_eq!(github.issues().len(), 1);
    github.set_pull_head(7, sha('a'));
    github.fail_next_comment_create_after_commit();
    block_on(comment_preflight(
        &github,
        &context(),
        7,
        "foo",
        &owner(1, 1, 'a'),
    ))
    .unwrap();
    assert_eq!(github.comments_for(7).len(), 1);
}

#[test]
fn reconciliation_errors_preserve_the_original_create_failure() {
    let github = FakeGitHub::new();
    github.fail_next_issue_create_after_commit();
    github.fail_issue_list();
    let error = block_on(issue_report(
        &github,
        &context(),
        &clock(1),
        &findings('a'),
        PublicationState::Findings,
    ))
    .unwrap_err();
    assert!(error.find_source::<AmbiguousCreateError>().is_some());
    let github = FakeGitHub::new();
    github.set_pull_head(7, sha('a'));
    github.fail_next_comment_create_after_commit();
    github.fail_comment_list();
    let error = block_on(comment_preflight(
        &github,
        &context(),
        7,
        "foo",
        &owner(1, 1, 'a'),
    ))
    .unwrap_err();
    assert!(error.find_source::<AmbiguousCreateError>().is_some());
}

#[test]
fn comment_failure_only_retires_its_exact_placeholder_and_can_be_restarted() {
    let github = FakeGitHub::new();
    github.set_pull_head(7, sha('a'));
    let pending = owner(2, 2, 'a');
    block_on(comment_failed(
        &github,
        &context(),
        7,
        &failure(pending.clone(), Conclusion::Failure),
    ))
    .unwrap();
    assert!(github.comments_for(7).is_empty());
    block_on(comment_preflight(&github, &context(), 7, "foo", &pending)).unwrap();
    let before = only_comment(&github);
    for obsolete in [owner(1, 2, 'a'), owner(2, 1, 'a'), owner(2, 2, 'b')] {
        block_on(comment_failed(
            &github,
            &context(),
            7,
            &failure(obsolete, Conclusion::Failure),
        ))
        .unwrap();
        assert_eq!(only_comment(&github), before);
    }
    block_on(comment_failed(
        &github,
        &context(),
        7,
        &failure(pending, Conclusion::Cancelled),
    ))
    .unwrap();
    assert!(only_comment(&github).body.contains("was cancelled"));
    block_on(comment_preflight(
        &github,
        &context(),
        7,
        "foo",
        &owner(2, 3, 'a'),
    ))
    .unwrap();
    let new = only_comment(&github);
    assert!(message::is_in_progress(&new.body, &context().instance));
    block_on(comment_preflight(
        &github,
        &context(),
        7,
        "foo",
        &owner(2, 2, 'a'),
    ))
    .unwrap();
    assert_eq!(only_comment(&github), new);
}

#[test]
fn completed_comment_states_survive_failure_and_empty_scope_can_restart() {
    for (outcome, complete, state) in [
        (Outcome::Findings, false, PublicationState::Findings),
        (Outcome::Clean, true, PublicationState::Clean),
        (Outcome::Partial, true, PublicationState::NoData),
        (
            Outcome::InsufficientBaseline,
            true,
            PublicationState::NoData,
        ),
    ] {
        let github = FakeGitHub::new();
        github.set_pull_head(7, sha('a'));
        let report = report(AnalysisMode::Branch, outcome, complete, owner(1, 1, 'a'));
        block_on(comment_report(
            &github,
            &context(),
            7,
            "foo",
            &report,
            state,
        ))
        .unwrap();
        let before = only_comment(&github);
        assert!(before.body.contains(&report.summary));
        assert!(
            !before
                .body
                .contains(&marker::stale_start(&context().instance))
        );
        block_on(comment_failed(
            &github,
            &context(),
            7,
            &failure(report.owner, Conclusion::Failure),
        ))
        .unwrap();
        assert_eq!(only_comment(&github), before);
    }
    let github = FakeGitHub::new();
    github.set_pull_head(7, sha('a'));
    block_on(comment_no_data(
        &github,
        &context(),
        7,
        None,
        &NoData::Empty(owner(1, 1, 'a')),
    ))
    .unwrap();
    assert!(
        only_comment(&github)
            .body
            .contains("No benchmarkable package")
    );
    block_on(comment_preflight(
        &github,
        &context(),
        7,
        "foo",
        &owner(2, 1, 'a'),
    ))
    .unwrap();
    assert!(message::is_in_progress(
        &only_comment(&github).body,
        &context().instance
    ));
}

#[test]
fn stale_comment_publication_warns_and_never_replaces_live_head_results() {
    let github = FakeGitHub::new();
    github.set_pull_head(7, sha('b'));
    github.set_comparison(&sha('a'), &sha('b'), Comparison { ahead_by: Some(1) });
    let old = report(
        AnalysisMode::Branch,
        Outcome::Findings,
        true,
        owner(1, 1, 'a'),
    );
    block_on(comment_report(
        &github,
        &context(),
        7,
        "foo",
        &old,
        PublicationState::Findings,
    ))
    .unwrap();
    assert!(only_comment(&github).body.contains("1 commit behind HEAD"));
    let current = report(AnalysisMode::Branch, Outcome::Clean, true, owner(1, 1, 'b'));
    block_on(comment_report(
        &github,
        &context(),
        7,
        "foo",
        &current,
        PublicationState::Clean,
    ))
    .unwrap();
    let before = only_comment(&github);
    block_on(comment_report(
        &github,
        &context(),
        7,
        "foo",
        &old,
        PublicationState::Findings,
    ))
    .unwrap();
    assert_eq!(only_comment(&github), before);
    block_on(comment_preflight(
        &github,
        &context(),
        7,
        "foo",
        &owner(2, 1, 'a'),
    ))
    .unwrap();
    block_on(comment_no_data(
        &github,
        &context(),
        7,
        None,
        &NoData::Empty(owner(2, 1, 'a')),
    ))
    .unwrap();
    assert_eq!(only_comment(&github), before);
}

#[test]
fn freshness_failure_qualifies_results_and_preflight_retains_completed_report() {
    let github = FakeGitHub::new();
    github.set_pull_head(7, sha('a'));
    let report = report(
        AnalysisMode::Branch,
        Outcome::Findings,
        true,
        owner(1, 1, 'a'),
    );
    block_on(comment_report(
        &github,
        &context(),
        7,
        "foo",
        &report,
        PublicationState::Findings,
    ))
    .unwrap();
    github.set_pull_head(7, sha('b'));
    block_on(comment_preflight(
        &github,
        &context(),
        7,
        "foo",
        &owner(2, 1, 'b'),
    ))
    .unwrap();
    assert!(only_comment(&github).body.contains(&report.summary));
    assert!(
        only_comment(&github)
            .body
            .contains("distance is unavailable")
    );
    github.fail_pull_head();
    block_on(comment_report(
        &github,
        &context(),
        7,
        "foo",
        &report,
        PublicationState::Findings,
    ))
    .unwrap();
    assert!(
        only_comment(&github)
            .body
            .contains("freshness could not be verified")
    );
}

#[test]
fn comment_state_mismatch_and_empty_scope_misuse_fail_before_lookup() {
    let github = FakeGitHub::new();
    let report = report(
        AnalysisMode::Branch,
        Outcome::Clean,
        false,
        owner(1, 1, 'a'),
    );
    block_on(comment_report(
        &github,
        &context(),
        7,
        "foo",
        &report,
        PublicationState::Clean,
    ))
    .unwrap_err();
    block_on(comment_no_data(
        &github,
        &context(),
        7,
        None,
        &NoData::Report(report),
    ))
    .unwrap_err();
    block_on(comment_no_data(
        &github,
        &context(),
        7,
        Some("foo"),
        &NoData::Empty(owner(1, 1, 'a')),
    ))
    .unwrap_err();
    block_on(comment_preflight(
        &github,
        &context(),
        7,
        "",
        &owner(1, 1, 'a'),
    ))
    .unwrap_err();
    assert!(github.comments_for(7).is_empty());
}

#[test]
fn renamed_rolling_titles_are_not_adopted() {
    let github = FakeGitHub::new();
    publish(&github, &findings('a'), 1);
    let old = only_issue(&github);
    block_on(github.update_issue(
        &context().repository,
        old.number,
        "Renamed by human",
        &old.body,
    ))
    .unwrap();
    block_on(issue_preflight(
        &github,
        &context(),
        &clock(2),
        &owner(2, 1, 'b'),
    ))
    .unwrap();
    assert_eq!(only_issue(&github).title, "Renamed by human");
}

#[test]
fn preflight_refreshes_pending_attempt_ownership() {
    let github = FakeGitHub::new();
    publish(&github, &findings('a'), 1);
    github.set_comparison(&sha('a'), &sha('c'), Comparison { ahead_by: Some(2) });
    block_on(issue_preflight(
        &github,
        &context(),
        &clock(2),
        &owner(2, 1, 'c'),
    ))
    .unwrap();
    block_on(issue_preflight(
        &github,
        &context(),
        &clock(3),
        &owner(2, 2, 'c'),
    ))
    .unwrap();
    let before = only_issue(&github);
    assert_eq!(
        IssueBody::parse(&before.body, &context().instance)
            .unwrap()
            .annotation
            .unwrap()
            .owner,
        owner(2, 2, 'c')
    );
}

#[test]
fn preflight_preserves_a_provably_newer_pending_head() {
    let github = FakeGitHub::new();
    publish(&github, &findings('a'), 1);
    github.set_comparison(&sha('a'), &sha('c'), Comparison { ahead_by: Some(2) });
    block_on(issue_preflight(
        &github,
        &context(),
        &clock(2),
        &owner(2, 2, 'c'),
    ))
    .unwrap();
    let before = only_issue(&github);
    github.set_comparison(&sha('a'), &sha('b'), Comparison { ahead_by: Some(1) });
    github.set_comparison(&sha('b'), &sha('c'), Comparison { ahead_by: Some(1) });
    block_on(issue_preflight(
        &github,
        &context(),
        &clock(4),
        &owner(3, 1, 'b'),
    ))
    .unwrap();
    assert_eq!(only_issue(&github), before);
}

#[test]
fn unowned_pr_comments_are_not_adopted_or_overwritten() {
    let github = FakeGitHub::new();
    github.set_pull_head(7, sha('a'));
    let human = block_on(github.create_comment(&context().repository, 7, "Human comment")).unwrap();
    block_on(comment_preflight(
        &github,
        &context(),
        7,
        "foo",
        &owner(1, 1, 'a'),
    ))
    .unwrap();
    let comments = github.comments_for(7);
    assert_eq!(comments.len(), 2);
    assert!(comments.contains(&human));
    assert_eq!(
        comments
            .iter()
            .filter(|comment| message::is_in_progress(&comment.body, &context().instance))
            .count(),
        1
    );
}

#[test]
fn zero_reverse_distance_does_not_prove_a_report_newer_than_preflight() {
    let github = FakeGitHub::new();
    publish(&github, &findings('a'), 1);
    github.set_comparison(&sha('b'), &sha('a'), Comparison { ahead_by: Some(0) });
    block_on(issue_preflight(
        &github,
        &context(),
        &clock(2),
        &owner(2, 1, 'b'),
    ))
    .unwrap();
    assert!(only_issue(&github).body.contains("distance is unavailable"));
}

#[test]
fn zero_reverse_distance_does_not_prove_an_annotation_newer_than_preflight() {
    let github = FakeGitHub::new();
    publish(&github, &findings('a'), 1);
    block_on(issue_preflight(
        &github,
        &context(),
        &clock(2),
        &owner(2, 1, 'c'),
    ))
    .unwrap();
    github.set_comparison(&sha('a'), &sha('b'), Comparison { ahead_by: Some(1) });
    github.set_comparison(&sha('b'), &sha('c'), Comparison { ahead_by: Some(0) });
    block_on(issue_preflight(
        &github,
        &context(),
        &clock(3),
        &owner(3, 1, 'b'),
    ))
    .unwrap();
    assert_eq!(
        IssueBody::parse(&only_issue(&github).body, &context().instance)
            .unwrap()
            .annotation
            .unwrap()
            .owner,
        owner(3, 1, 'b')
    );
}

#[test]
fn ownership_order_is_strict_and_attempts_are_ordered_within_the_run() {
    let incoming = owner(42, 2, 'a');
    assert!(!superseded(&incoming, &incoming));
    assert!(!superseded(&owner(42, 1, 'a'), &incoming));
    assert!(superseded(&owner(42, 3, 'a'), &incoming));
    assert!(superseded(&owner(43, 1, 'a'), &incoming));
    assert!(!superseded(&owner(41, 3, 'a'), &incoming));
}

#[test]
fn zero_distance_for_distinct_commits_does_not_authorize_report_replacement() {
    let github = FakeGitHub::new();
    publish(&github, &findings('a'), 1);
    let before = only_issue(&github);
    github.set_comparison(&sha('a'), &sha('b'), Comparison { ahead_by: Some(0) });
    publish(
        &github,
        &report(
            AnalysisMode::History,
            Outcome::Clean,
            true,
            owner(2, 1, 'b'),
        ),
        2,
    );
    assert_eq!(only_issue(&github), before);
}

#[test]
fn issue_body_requires_matching_identity_valid_state_and_consistent_commit() {
    let context = context();
    let report = findings('a');
    let body = message::regression_issue(
        &context.instance,
        &report.owner,
        &report.evidence,
        &report.summary,
        None,
    );
    for invalid in [
        body.replace(
            &marker::issue(&context.instance, IssueKind::Regression),
            &marker::pr_comment(&context.instance),
        ),
        body.replace(
            &marker::state(&context.instance, "findings"),
            &marker::state(&context.instance, "unknown"),
        ),
        body.replace(
            &marker::run_owner(&context.instance, &report.owner),
            &marker::run_owner(&context.instance, &owner(1, 1, 'b')),
        ),
    ] {
        let error = IssueBody::parse(&invalid, &context.instance).err().unwrap();
        assert!(error.find_source::<UninterpretableIssue>().is_some());
    }
}

#[test]
fn duplicated_annotation_boundaries_are_uninterpretable() {
    let context = context();
    let report = findings('a');
    let body = message::regression_issue(
        &context.instance,
        &report.owner,
        &report.evidence,
        &report.summary,
        None,
    );
    let body = annotate(
        &body,
        &context.instance,
        &owner(2, 1, 'a'),
        "preflight",
        "Pending",
    );
    for boundary in [
        marker::annotation_start(&context.instance),
        marker::annotation_end(&context.instance),
    ] {
        let broken = body.replace(&boundary, &format!("{boundary}\n{boundary}"));
        let error = IssueBody::parse(&broken, &context.instance).err().unwrap();
        assert!(error.find_source::<UninterpretableIssue>().is_some());
    }
}

#[test]
fn empty_issue_scope_annotates_without_replacing_the_report() {
    let github = FakeGitHub::new();
    publish(&github, &findings('a'), 1);
    let before = only_issue(&github);
    block_on(issue_no_data(
        &github,
        &context(),
        &clock(2),
        &NoData::Empty(owner(2, 1, 'a')),
    ))
    .unwrap();
    let after = only_issue(&github);
    assert_eq!(
        IssueBody::parse(&after.body, &context().instance)
            .unwrap()
            .report,
        before.body
    );
    assert!(
        after
            .body
            .contains("No benchmarkable packages were selected")
    );
}

#[test]
fn successful_partial_analysis_uses_comment_no_data_publication() {
    let github = FakeGitHub::new();
    github.set_pull_head(7, sha('a'));
    let partial = report(
        AnalysisMode::Branch,
        Outcome::Clean,
        false,
        owner(1, 1, 'a'),
    );
    block_on(comment_no_data(
        &github,
        &context(),
        7,
        Some("foo"),
        &NoData::Report(partial),
    ))
    .unwrap();
    let body = only_comment(&github).body;
    assert!(body.contains("completed platforms only"));
    assert_eq!(
        marker::find_state(&body, &context().instance),
        Some("no-data")
    );
}

#[test]
fn run_head_only_placeholders_are_not_adopted() {
    let github = FakeGitHub::new();
    github.set_pull_head(7, sha('a'));
    let context = context();
    let body = format!(
        "{}\n{}\n<!-- cargo-bench-history:project:run:42:{} -->",
        marker::pr_comment(&context.instance),
        marker::in_progress(&context.instance),
        sha('a').as_str()
    );
    let existing = block_on(github.create_comment(&context.repository, 7, &body)).unwrap();
    block_on(comment_preflight(
        &github,
        &context,
        7,
        "foo",
        &owner(42, 2, 'a'),
    ))
    .unwrap_err();
    assert_eq!(only_comment(&github), existing);
}

#[test]
fn comment_metadata_requires_a_recognized_state_and_matching_report_commit() {
    let instance = &context().instance;
    let prefix = format!(
        "{}\n{}",
        marker::pr_comment(instance),
        marker::run_owner(instance, &owner(1, 1, 'a'))
    );
    for metadata in [
        String::new(),
        marker::analyzed_sha(instance, &sha('a')),
        marker::state(instance, "unknown"),
        format!(
            "{}\n{}",
            marker::state(instance, "unknown"),
            marker::analyzed_sha(instance, &sha('a'))
        ),
        marker::state(instance, "findings"),
        format!(
            "{}\n{}\n{}",
            marker::state(instance, "clean"),
            marker::state(instance, "clean"),
            marker::analyzed_sha(instance, &sha('a'))
        ),
        format!(
            "{}\n{}",
            marker::state(instance, "findings"),
            marker::analyzed_sha(instance, &sha('b'))
        ),
    ] {
        assert_comment_metadata_rejected(&format!("{prefix}\n{metadata}"));
    }
}

#[test]
fn comment_metadata_rejects_duplicated_or_mixed_note_and_report_states() {
    let instance = &context().instance;
    let pending = message::pr_in_progress(instance, "foo", &owner(1, 1, 'a'));
    for extra in [
        marker::pr_comment(instance),
        marker::in_progress(instance),
        marker::empty_scope(instance),
        marker::failed(instance),
        marker::state(instance, "clean"),
        format!(
            "{}\n{}",
            marker::state(instance, "clean"),
            marker::state(instance, "clean")
        ),
        marker::analyzed_sha(instance, &sha('a')),
        "<!-- cargo-bench-history:project:analyzed-sha:invalid -->".to_owned(),
    ] {
        assert_comment_metadata_rejected(&format!("{pending}\n{extra}"));
    }
}

#[test]
fn no_data_retires_the_pending_head_when_the_retained_report_distance_is_unknown() {
    let github = FakeGitHub::new();
    publish(&github, &findings('a'), 1);
    block_on(issue_preflight(
        &github,
        &context(),
        &clock(2),
        &owner(2, 1, 'b'),
    ))
    .unwrap();
    let before = only_issue(&github);
    let partial = report(
        AnalysisMode::History,
        Outcome::Partial,
        true,
        owner(2, 2, 'b'),
    );
    block_on(issue_no_data(
        &github,
        &context(),
        &clock(3),
        &NoData::Report(partial),
    ))
    .unwrap();
    let after = only_issue(&github);
    let parsed = IssueBody::parse(&after.body, &context().instance).unwrap();
    assert_eq!(
        parsed.report,
        IssueBody::parse(&before.body, &context().instance)
            .unwrap()
            .report
    );
    assert!(
        parsed
            .annotation
            .is_some_and(|annotation| annotation.state == AnnotationState::NoData)
    );
    assert!(after.body.contains("distance is unavailable"));
}

#[test]
fn no_data_cannot_retire_a_newer_attempt_at_the_same_pending_head() {
    let github = FakeGitHub::new();
    publish(&github, &findings('a'), 1);
    block_on(issue_preflight(
        &github,
        &context(),
        &clock(2),
        &owner(2, 2, 'b'),
    ))
    .unwrap();
    let before = only_issue(&github);
    block_on(issue_no_data(
        &github,
        &context(),
        &clock(3),
        &NoData::Empty(owner(2, 1, 'b')),
    ))
    .unwrap();
    assert_eq!(only_issue(&github), before);
}
