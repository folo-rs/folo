use futures::executor::block_on;

use crate::cli::{Conclusion, PendingArgs};
use crate::github::fake::FakeGitHub;
use crate::github::{Comparison, Issue};
use crate::lifecycle::tests::harness::{
    clock, context, failure, findings, only_issue, owner, publish, report, sha,
};
use crate::lifecycle::{
    IssueBody, UninterpretableIssue, alert, annotate, comment_failed, issue_failed, issue_preflight,
};
use crate::result::{AnalysisMode, Outcome};

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
fn persisted_alerts_preserve_open_and_closed_content() {
    for open in [true, false] {
        let github = FakeGitHub::new();
        let issue = Issue {
            number: 7,
            title: "Benchmark history workflow failed for project (run 42)".to_owned(),
            // Independent persisted input protects discovery from a coupled formatter change.
            body: "<!-- cargo-bench-history:project:issue:failure-alert -->\n\
                <!-- cargo-bench-history:project:alert-run:42 -->\n\
                Retained investigation"
                .to_owned(),
            open,
        };
        github.seed_issue(issue.clone());
        let args = failure(owner(42, 1, 'a'), Conclusion::Failure);
        block_on(alert(
            &github,
            &context(),
            args.pending.run.run_id,
            &args.run_url,
        ))
        .unwrap();
        assert_eq!(only_issue(&github), issue);
    }
}

#[test]
fn persisted_alert_body_must_match_project_run_and_kind() {
    let args = failure(owner(42, 1, 'a'), Conclusion::Failure);
    for (project, run, kind) in [
        ("project.extra", 42, "failure-alert"),
        ("project", 43, "failure-alert"),
        ("project", 42, "regression"),
    ] {
        let github = FakeGitHub::new();
        let issue = Issue {
            number: 7,
            title: "Benchmark history workflow failed for project (run 42)".to_owned(),
            body: format!(
                "<!-- cargo-bench-history:project:issue:{kind} -->\n\
                <!-- cargo-bench-history:{project}:alert-run:{run} -->"
            ),
            open: true,
        };
        github.seed_issue(issue.clone());
        let error = block_on(alert(
            &github,
            &context(),
            args.pending.run.run_id,
            &args.run_url,
        ))
        .unwrap_err();
        assert!(error.find_source::<UninterpretableIssue>().is_some());
        assert_eq!(only_issue(&github), issue);
    }
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
