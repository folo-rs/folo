use futures::executor::block_on;

use crate::cli::Conclusion;
use crate::github::GitHub;
use crate::github::fake::FakeGitHub;
use crate::lifecycle::tests::harness::{context, failure, only_comment, owner, report, sha};
use crate::lifecycle::{
    Inconclusive, comment_failed, comment_inconclusive, comment_preflight, comment_report,
};
use crate::result::{AnalysisMode, Outcome, PublicationState, WrongPublicationState};
use crate::{marker, message};

#[test]
fn whitespace_summary_cannot_publish_comment_findings() {
    let github = FakeGitHub::new();
    github.set_pull_head(7, sha('a'));
    let mut report = report(
        AnalysisMode::Branch,
        Outcome::Findings,
        true,
        owner(1, 1, 'a'),
    );
    report.summary = " \r\n\t ".to_owned();
    block_on(comment_report(
        &github,
        &context(),
        7,
        "foo",
        &report,
        PublicationState::Findings,
    ))
    .unwrap_err();
    assert!(github.comments_for(7).is_empty());
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
    for (outcome, complete, state, marker_state) in [
        (
            Outcome::Findings,
            false,
            PublicationState::Findings,
            "findings",
        ),
        (Outcome::Clean, true, PublicationState::Clean, "clean"),
        (
            Outcome::Partial,
            true,
            PublicationState::Inconclusive,
            "no-data",
        ),
        (
            Outcome::InsufficientBaseline,
            true,
            PublicationState::Inconclusive,
            "no-data",
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
        assert_eq!(
            marker::find_state(&before.body, &context().instance),
            Some(marker_state)
        );
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
    block_on(comment_inconclusive(
        &github,
        &context(),
        7,
        None,
        &Inconclusive::Empty(owner(1, 1, 'a')),
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
    block_on(comment_inconclusive(
        &github,
        &context(),
        7,
        None,
        &Inconclusive::Report(report),
    ))
    .unwrap_err();
    block_on(comment_inconclusive(
        &github,
        &context(),
        7,
        Some("foo"),
        &Inconclusive::Empty(owner(1, 1, 'a')),
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
fn distinct_run_comment_preflight_accepts_a_lower_run_identifier() {
    let github = FakeGitHub::new();
    let context = context();
    github.set_pull_head(7, sha('a'));
    let body = message::pr_in_progress(&context.instance, "foo", &owner(43, 3, 'a'));
    block_on(github.create_comment(&context.repository, 7, &body)).unwrap();
    let incoming = owner(42, 1, 'a');
    block_on(comment_preflight(&github, &context, 7, "foo", &incoming)).unwrap();
    assert_eq!(
        marker::find_owner(&only_comment(&github).body, &context.instance),
        Some(incoming)
    );
}

#[test]
fn distinct_run_failure_cannot_retire_a_same_commit_placeholder() {
    let github = FakeGitHub::new();
    let context = context();
    let body = message::pr_in_progress(&context.instance, "foo", &owner(42, 1, 'a'));
    let before = block_on(github.create_comment(&context.repository, 7, &body)).unwrap();
    block_on(comment_failed(
        &github,
        &context,
        7,
        &failure(owner(43, 3, 'a'), Conclusion::Failure),
    ))
    .unwrap();
    assert_eq!(only_comment(&github), before);
}

#[test]
fn successful_partial_analysis_uses_comment_inconclusive_publication() {
    let github = FakeGitHub::new();
    github.set_pull_head(7, sha('a'));
    let partial = report(
        AnalysisMode::Branch,
        Outcome::Clean,
        false,
        owner(1, 1, 'a'),
    );
    block_on(comment_inconclusive(
        &github,
        &context(),
        7,
        Some("foo"),
        &Inconclusive::Report(partial),
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
fn inconclusive_comment_rejects_clean_evidence_without_replacing_existing_state() {
    let github = FakeGitHub::new();
    let context = context();
    github.set_pull_head(7, sha('a'));
    block_on(comment_preflight(
        &github,
        &context,
        7,
        "foo",
        &owner(1, 1, 'a'),
    ))
    .unwrap();
    let before = only_comment(&github);
    let report = report(AnalysisMode::Branch, Outcome::Clean, true, owner(1, 1, 'a'));
    let error = block_on(comment_inconclusive(
        &github,
        &context,
        7,
        Some("foo"),
        &Inconclusive::Report(report),
    ))
    .unwrap_err();
    assert!(error.find_source::<WrongPublicationState>().is_some());
    assert_eq!(only_comment(&github), before);
}
