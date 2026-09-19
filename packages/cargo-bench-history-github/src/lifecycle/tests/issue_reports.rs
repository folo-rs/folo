use futures::executor::block_on;

use crate::cli::Conclusion;
use crate::github::Comparison;
use crate::github::fake::FakeGitHub;
use crate::lifecycle::tests::harness::{
    clock, context, failure, findings, only_issue, owner, publish, report, sha,
};
use crate::lifecycle::{NoData, issue_failed, issue_no_data, issue_preflight, issue_report};
use crate::marker;
use crate::result::{AnalysisMode, Coverage, Outcome, PublicationState, WrongPublicationState};

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
fn empty_summary_cannot_publish_issue_findings() {
    let github = FakeGitHub::new();
    let mut report = findings('a');
    report.summary.clear();
    block_on(issue_report(
        &github,
        &context(),
        &clock(1),
        &report,
        PublicationState::Findings,
    ))
    .unwrap_err();
    assert!(github.issues().is_empty());
}
