use futures::executor::block_on;

use crate::cli::{Conclusion, PendingArgs};
use crate::github::fake::FakeGitHub;
use crate::github::{Comparison, GitHub};
use crate::lifecycle::tests::harness::{context, failure, only_comment, owner, report, sha};
use crate::lifecycle::{NoData, comment_no_data, comment_report};
use crate::result::{AnalysisMode, Outcome, PublicationState};
use crate::{marker, message};

fn branch_result_body(owner: PendingArgs, outcome: Outcome) -> String {
    let report = report(AnalysisMode::Branch, outcome, true, owner);
    message::pr_result(
        &context().instance,
        &report.owner,
        &report.evidence,
        "foo",
        &report.summary,
        None,
    )
}

fn assert_stale_report_preserves_non_live_state(
    body: &str,
    previous_to_incoming: Option<u64>,
    incoming_to_previous: Option<u64>,
) {
    let github = FakeGitHub::new();
    let context = context();
    github.set_pull_head(7, sha('c'));
    github.set_comparison(
        &sha('b'),
        &sha('a'),
        Comparison {
            ahead_by: previous_to_incoming,
        },
    );
    github.set_comparison(
        &sha('a'),
        &sha('b'),
        Comparison {
            ahead_by: incoming_to_previous,
        },
    );
    let before = block_on(github.create_comment(&context.repository, 7, body)).unwrap();
    // The incoming run has larger identifiers but an older or unorderable commit.
    let incoming = report(
        AnalysisMode::Branch,
        Outcome::Clean,
        true,
        owner(43, 3, 'a'),
    );
    block_on(comment_report(
        &github,
        &context,
        7,
        "foo",
        &incoming,
        PublicationState::Clean,
    ))
    .unwrap();
    assert_eq!(only_comment(&github), before);
}

#[test]
fn stale_report_preserves_newer_non_live_report() {
    let body = branch_result_body(owner(42, 1, 'b'), Outcome::Findings);
    assert_stale_report_preserves_non_live_state(&body, None, Some(1));
}

#[test]
fn stale_report_preserves_newer_non_live_no_data_report() {
    let body = branch_result_body(owner(42, 1, 'b'), Outcome::Partial);
    assert_stale_report_preserves_non_live_state(&body, None, Some(1));
}

#[test]
fn stale_report_preserves_newer_non_live_pending_note() {
    let body = message::pr_in_progress(&context().instance, "foo", &owner(42, 1, 'b'));
    assert_stale_report_preserves_non_live_state(&body, None, Some(1));
}

#[test]
fn stale_report_preserves_newer_non_live_failed_note() {
    let args = failure(owner(42, 1, 'b'), Conclusion::Failure);
    let body = message::pr_failed(
        &context().instance,
        &args.pending,
        &args.run_url,
        args.conclusion,
    );
    assert_stale_report_preserves_non_live_state(&body, None, Some(1));
}

#[test]
fn stale_report_preserves_newer_non_live_empty_scope_note() {
    let body = message::pr_nothing_in_scope(&context().instance, &owner(42, 1, 'b'));
    assert_stale_report_preserves_non_live_state(&body, None, Some(1));
}

#[test]
fn stale_report_preserves_non_live_state_when_commit_order_is_unknown() {
    let body = branch_result_body(owner(42, 1, 'b'), Outcome::Findings);
    assert_stale_report_preserves_non_live_state(&body, None, None);
}

#[test]
fn stale_report_preserves_distinct_non_live_head_on_zero_distance() {
    let body = branch_result_body(owner(42, 1, 'b'), Outcome::Findings);
    assert_stale_report_preserves_non_live_state(&body, Some(0), None);
}

fn assert_comment_report_replaces_previous(
    previous_head: char,
    incoming_head: char,
    live_head: char,
    forward_distance: Option<u64>,
) {
    let github = FakeGitHub::new();
    let context = context();
    github.set_pull_head(7, sha(live_head));
    github.set_comparison(
        &sha(previous_head),
        &sha(incoming_head),
        Comparison {
            ahead_by: forward_distance,
        },
    );
    let body = branch_result_body(owner(43, 3, previous_head), Outcome::Clean);
    let before = block_on(github.create_comment(&context.repository, 7, &body)).unwrap();
    let incoming = report(
        AnalysisMode::Branch,
        Outcome::Findings,
        true,
        owner(42, 1, incoming_head),
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

#[test]
fn stale_report_advances_a_non_live_comment_by_verified_commit_order() {
    assert_comment_report_replaces_previous('a', 'b', 'c', Some(1));
}

#[test]
fn distinct_run_reports_at_the_same_stale_head_follow_publication_order() {
    assert_comment_report_replaces_previous('b', 'b', 'c', None);
}

#[test]
fn live_head_report_replaces_unrelated_previous_state() {
    assert_comment_report_replaces_previous('a', 'b', 'b', None);
}

#[test]
fn stale_no_data_report_preserves_newer_non_live_state() {
    let github = FakeGitHub::new();
    let context = context();
    github.set_pull_head(7, sha('c'));
    github.set_comparison(&sha('a'), &sha('b'), Comparison { ahead_by: Some(1) });
    let body = branch_result_body(owner(42, 1, 'b'), Outcome::Findings);
    let before = block_on(github.create_comment(&context.repository, 7, &body)).unwrap();
    let incoming = report(
        AnalysisMode::Branch,
        Outcome::Partial,
        true,
        owner(43, 3, 'a'),
    );
    block_on(comment_no_data(
        &github,
        &context,
        7,
        Some("foo"),
        &NoData::Report(incoming),
    ))
    .unwrap();
    assert_eq!(only_comment(&github), before);
}

#[test]
fn unavailable_live_head_retains_qualified_publication_for_different_heads() {
    let github = FakeGitHub::new();
    let context = context();
    let body = branch_result_body(owner(43, 3, 'b'), Outcome::Clean);
    block_on(github.create_comment(&context.repository, 7, &body)).unwrap();
    github.fail_pull_head();
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
    let after = only_comment(&github);
    assert_eq!(
        marker::find_owner(&after.body, &context.instance),
        Some(incoming.owner)
    );
    assert!(after.body.contains("freshness could not be verified"));
}
