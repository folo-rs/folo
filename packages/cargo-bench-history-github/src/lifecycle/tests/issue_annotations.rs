use futures::executor::block_on;

use crate::github::fake::FakeGitHub;
use crate::github::{Comparison, Issue};
use crate::lifecycle::tests::harness::{
    clock, context, findings, only_issue, owner, publish, report, seed_retained_findings, sha,
};
use crate::lifecycle::{
    AnnotationState, Inconclusive, IssueBody, issue_inconclusive, issue_preflight,
};
use crate::marker;
use crate::result::{AnalysisMode, Outcome};

#[test]
fn persisted_annotation_is_replaced_without_rewriting_the_report() {
    let github = FakeGitHub::new();
    // These are persisted wire bodies, deliberately independent of the production formatters.
    let retained = "<!-- cargo-bench-history:project:issue:regression -->\n\
        <!-- cargo-bench-history:project:run:1:1:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa -->\n\
        <!-- cargo-bench-history:project:analyzed-sha:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa -->\n\
        <!-- cargo-bench-history:project:state:findings -->\n\n\
        Retained report with its original whitespace.\n";
    let start = "<!-- cargo-bench-history:project:annotation:start -->";
    let end = "<!-- cargo-bench-history:project:annotation:end -->";
    github.seed_issue(Issue {
        number: 7,
        title: "Benchmark history findings for project (updated 2026-01-01)".to_owned(),
        body: format!(
            "{retained}\n\n{start}\n\
            <!-- cargo-bench-history:project:run:2:1:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb -->\n\
            <!-- cargo-bench-history:project:state:preflight -->\n\
            Superseded annotation\n{end}"
        ),
        open: true,
    });
    let incoming = owner(2, 2, 'b');
    block_on(issue_inconclusive(
        &github,
        &context(),
        &clock(2),
        &Inconclusive::Empty(incoming.clone()),
    ))
    .unwrap();
    let after = only_issue(&github);
    let parsed = IssueBody::parse(&after.body, &context().instance).unwrap();
    assert_eq!(parsed.report, retained);
    assert_eq!(parsed.owner, owner(1, 1, 'a'));
    assert_eq!(parsed.commit, sha('a'));
    let annotation = parsed.annotation.unwrap();
    assert_eq!(annotation.owner, incoming);
    assert!(annotation.state == AnnotationState::Inconclusive);
    assert_eq!(after.body.matches(start).count(), 1);
    assert_eq!(after.body.matches(end).count(), 1);
    assert!(!after.body.contains("Superseded annotation"));
    assert!(after.open);
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
fn inconclusive_retains_report_commit_and_staleness_and_replaces_one_annotation() {
    let github = FakeGitHub::new();
    seed_retained_findings(&github, &owner(1, 1, 'a'));
    github.set_comparison(&sha('a'), &sha('b'), Comparison { ahead_by: Some(2) });
    let pending = owner(2, 1, 'b');
    block_on(issue_preflight(&github, &context(), &clock(2), &pending)).unwrap();
    let pending_body = only_issue(&github);
    let retained = IssueBody::parse(&pending_body.body, &context().instance)
        .unwrap()
        .report
        .to_owned();
    // A later unavailable comparison must not degrade this pending head's retained warning.
    github.set_comparison(&sha('a'), &sha('b'), Comparison { ahead_by: None });
    let partial = report(AnalysisMode::History, Outcome::Clean, false, pending);
    block_on(issue_inconclusive(
        &github,
        &context(),
        &clock(3),
        &Inconclusive::Report(partial),
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
    assert!(parsed.annotation.unwrap().state == AnnotationState::Inconclusive);
    assert_ne!(after.title, pending_body.title);
}

#[test]
fn inconclusive_without_preflight_qualifies_the_retained_report() {
    let github = FakeGitHub::new();
    publish(&github, &findings('a'), 1);
    github.set_comparison(&sha('a'), &sha('b'), Comparison { ahead_by: Some(2) });
    let incoming = report(
        AnalysisMode::History,
        Outcome::InsufficientBaseline,
        true,
        owner(2, 1, 'b'),
    );
    block_on(issue_inconclusive(
        &github,
        &context(),
        &clock(2),
        &Inconclusive::Report(incoming),
    ))
    .unwrap();
    let issue = only_issue(&github);
    let parsed = IssueBody::parse(&issue.body, &context().instance).unwrap();
    assert_eq!(parsed.commit, sha('a'));
    assert!(parsed.report.contains("2 commits behind HEAD"));
    assert!(parsed.report.contains(&findings('a').summary));
    let annotation = parsed.annotation.unwrap();
    assert_eq!(annotation.owner, owner(2, 1, 'b'));
    assert!(matches!(annotation.state, AnnotationState::Inconclusive));
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
fn empty_issue_scope_annotates_without_replacing_the_report() {
    let github = FakeGitHub::new();
    publish(&github, &findings('a'), 1);
    let before = only_issue(&github);
    block_on(issue_inconclusive(
        &github,
        &context(),
        &clock(2),
        &Inconclusive::Empty(owner(2, 1, 'a')),
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
fn inconclusive_retires_the_pending_head_when_the_retained_report_distance_is_unknown() {
    let github = FakeGitHub::new();
    seed_retained_findings(&github, &owner(1, 1, 'a'));
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
    block_on(issue_inconclusive(
        &github,
        &context(),
        &clock(3),
        &Inconclusive::Report(partial),
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
            .is_some_and(|annotation| annotation.state == AnnotationState::Inconclusive)
    );
    assert!(after.body.contains("distance is unavailable"));
}

#[test]
fn inconclusive_cannot_retire_a_newer_attempt_at_the_same_pending_head() {
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
    block_on(issue_inconclusive(
        &github,
        &context(),
        &clock(3),
        &Inconclusive::Empty(owner(2, 1, 'b')),
    ))
    .unwrap();
    assert_eq!(only_issue(&github), before);
}
