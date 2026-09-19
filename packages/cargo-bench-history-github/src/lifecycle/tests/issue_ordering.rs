use futures::executor::block_on;

use crate::github::Comparison;
use crate::github::fake::FakeGitHub;
use crate::lifecycle::tests::harness::{
    clock, context, findings, only_issue, owner, publish, report, seed_retained_findings, sha,
};
use crate::lifecycle::{IssueBody, NoData, issue_no_data, issue_preflight, superseded};
use crate::marker;
use crate::result::{AnalysisMode, Outcome};

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
    assert!(!superseded(&owner(43, 1, 'a'), &incoming));
    assert!(!superseded(&owner(43, 3, 'a'), &incoming));
    assert!(!superseded(&owner(41, 3, 'a'), &incoming));
}

#[test]
fn distinct_run_issue_reports_at_the_same_commit_follow_publication_order() {
    let github = FakeGitHub::new();
    seed_retained_findings(&github, &owner(43, 3, 'a'));
    let incoming = report(
        AnalysisMode::History,
        Outcome::Clean,
        true,
        owner(42, 1, 'a'),
    );
    publish(&github, &incoming, 2);
    let issue = only_issue(&github);
    let parsed = IssueBody::parse(&issue.body, &context().instance).unwrap();
    assert_eq!(parsed.owner, incoming.owner);
    assert_eq!(parsed.commit, sha('a'));
    assert_eq!(
        marker::find_state(parsed.report, &context().instance),
        Some("clean")
    );
}

#[test]
fn distinct_run_issue_reports_advance_by_commit_not_identifier() {
    let github = FakeGitHub::new();
    seed_retained_findings(&github, &owner(43, 3, 'a'));
    let incoming = report(
        AnalysisMode::History,
        Outcome::Findings,
        true,
        owner(42, 1, 'b'),
    );
    github.set_comparison(&sha('a'), &sha('b'), Comparison { ahead_by: Some(1) });
    publish(&github, &incoming, 2);
    let issue = only_issue(&github);
    let parsed = IssueBody::parse(&issue.body, &context().instance).unwrap();
    assert_eq!(parsed.owner, incoming.owner);
    assert_eq!(parsed.commit, sha('b'));
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
