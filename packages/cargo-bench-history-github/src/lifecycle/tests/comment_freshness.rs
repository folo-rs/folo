use futures::executor::block_on;

use crate::cli::Conclusion;
use crate::github::fake::FakeGitHub;
use crate::github::{Comparison, GitHub};
use crate::lifecycle::tests::harness::{context, failure, only_comment, owner, report, sha};
use crate::lifecycle::{Inconclusive, comment_inconclusive, comment_preflight, comment_report};
use crate::result::{AnalysisMode, Outcome, PublicationState};
use crate::{marker, message};

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
    block_on(comment_inconclusive(
        &github,
        &context(),
        7,
        None,
        &Inconclusive::Empty(owner(2, 1, 'a')),
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
fn distinct_run_comment_reports_at_the_same_live_head_follow_publication_order() {
    let github = FakeGitHub::new();
    let context = context();
    github.set_pull_head(7, sha('a'));
    let previous = report(
        AnalysisMode::Branch,
        Outcome::Clean,
        true,
        owner(43, 3, 'a'),
    );
    let body = message::pr_result(
        &context.instance,
        &previous.owner,
        &previous.evidence,
        previous.evidence.publication_state(),
        "foo",
        &previous.summary,
        None,
    );
    block_on(github.create_comment(&context.repository, 7, &body)).unwrap();
    let incoming = report(
        AnalysisMode::Branch,
        Outcome::Findings,
        true,
        owner(42, 1, 'a'),
    );
    block_on(comment_report(
        &github,
        &context,
        7,
        "foo",
        &incoming,
        PublicationState::Findings,
    ))
    .unwrap();
    assert_eq!(
        marker::find_owner(&only_comment(&github).body, &context.instance),
        Some(incoming.owner)
    );
}

#[test]
fn distinct_run_stale_comment_preserves_the_live_head_report() {
    let github = FakeGitHub::new();
    let context = context();
    github.set_pull_head(7, sha('b'));
    let current = report(
        AnalysisMode::Branch,
        Outcome::Clean,
        true,
        owner(42, 1, 'b'),
    );
    let body = message::pr_result(
        &context.instance,
        &current.owner,
        &current.evidence,
        current.evidence.publication_state(),
        "foo",
        &current.summary,
        None,
    );
    let before = block_on(github.create_comment(&context.repository, 7, &body)).unwrap();
    let incoming = report(
        AnalysisMode::Branch,
        Outcome::Findings,
        true,
        owner(43, 3, 'a'),
    );
    block_on(comment_report(
        &github,
        &context,
        7,
        "foo",
        &incoming,
        PublicationState::Findings,
    ))
    .unwrap();
    assert_eq!(only_comment(&github), before);
}

fn assert_stale_report_preserves_live_head_note(body: &str) {
    let github = FakeGitHub::new();
    let context = context();
    github.set_pull_head(7, sha('b'));
    let before = block_on(github.create_comment(&context.repository, 7, body)).unwrap();
    // Run identifiers deliberately disagree with head freshness.
    let incoming = report(
        AnalysisMode::Branch,
        Outcome::Findings,
        true,
        owner(43, 3, 'a'),
    );
    block_on(comment_report(
        &github,
        &context,
        7,
        "foo",
        &incoming,
        PublicationState::Findings,
    ))
    .unwrap();
    assert_eq!(only_comment(&github), before);
}

#[test]
fn stale_report_preserves_live_head_pending_note() {
    let body = message::pr_in_progress(&context().instance, "foo", &owner(42, 1, 'b'));
    assert_stale_report_preserves_live_head_note(&body);
}

#[test]
fn stale_report_preserves_live_head_failed_note() {
    let args = failure(owner(42, 1, 'b'), Conclusion::Failure);
    let body = message::pr_failed(
        &context().instance,
        &args.pending,
        &args.run_url,
        args.conclusion,
    );
    assert_stale_report_preserves_live_head_note(&body);
}

#[test]
fn stale_report_preserves_live_head_empty_scope_note() {
    let body = message::pr_nothing_in_scope(&context().instance, &owner(42, 1, 'b'));
    assert_stale_report_preserves_live_head_note(&body);
}

#[test]
fn same_head_report_replaces_a_live_head_note() {
    let github = FakeGitHub::new();
    let context = context();
    github.set_pull_head(7, sha('b'));
    let args = failure(owner(43, 3, 'b'), Conclusion::Failure);
    let body = message::pr_failed(
        &context.instance,
        &args.pending,
        &args.run_url,
        args.conclusion,
    );
    let before = block_on(github.create_comment(&context.repository, 7, &body)).unwrap();
    let incoming = report(
        AnalysisMode::Branch,
        Outcome::Findings,
        true,
        owner(42, 1, 'b'),
    );
    block_on(comment_report(
        &github,
        &context,
        7,
        "foo",
        &incoming,
        PublicationState::Findings,
    ))
    .unwrap();
    let after = only_comment(&github);
    assert_eq!(after.id, before.id);
    assert_eq!(
        marker::find_owner(&after.body, &context.instance),
        Some(incoming.owner)
    );
    assert_eq!(
        marker::find_state(&after.body, &context.instance),
        Some("findings")
    );
}
