use futures::executor::block_on;

use crate::github::fake::FakeGitHub;
use crate::github::{Comparison, GitHub, Issue};
use crate::marker;
use crate::message::{self, Envelope};
use crate::migration::{
    AlreadyManagedLegacyIssue, AmbiguousLegacyIssue, InvalidLegacyIssueTitle, MigrationOptions,
    validate_legacy_title,
};
use crate::model::{CommitSha, IssueKind};
use crate::operations::{
    Context, alert, issue_cleanup, issue_preflight, pr_comment_finalize, pr_comment_preflight,
    publish_issue, resolve_alert,
};
use crate::result::tests::evidence;
use crate::result::{AnalysisMode, Outcome, UnsafeAllClear};

fn context() -> Context {
    Context {
        repository: "owner/repo".parse().unwrap(),
        instance: "current".parse().unwrap(),
        verbose: false,
        comment_marker: None,
        migration: MigrationOptions {
            issue_title: Some("Legacy automation issue".to_owned()),
            in_progress_marker: None,
        },
    }
}

fn legacy_issue(github: &FakeGitHub, context: &Context, body: &str) -> Issue {
    block_on(github.create_issue(
        &context.repository,
        context.migration.issue_title.as_deref().unwrap(),
        body,
    ))
    .unwrap()
}

fn only_issue(github: &FakeGitHub) -> Issue {
    let issues = github.issues();
    assert_eq!(issues.len(), 1);
    issues.into_iter().next().unwrap()
}

fn sha(value: char) -> CommitSha {
    value.to_string().repeat(40).parse().unwrap()
}

#[test]
fn preflight_preserves_legacy_content_until_validated_publication_adopts_it() {
    let github = FakeGitHub::new();
    let context = context();
    let old = legacy_issue(
        &github,
        &context,
        "Original report with arbitrary commit prose.",
    );
    for head in [sha('a'), sha('b')] {
        block_on(issue_preflight(&github, &context, &head)).unwrap();
        let stale = only_issue(&github);
        assert_eq!(stale.number, old.number);
        assert_eq!(stale.title, old.title);
        assert!(stale.body.contains(&old.body));
        assert!(!marker::has_issue_identity(&stale.body));
        assert!(stale.body.contains(&marker::stale_start(&context.instance)));
    }
    let report = evidence(AnalysisMode::History, Outcome::Findings, true);
    block_on(publish_issue(
        &github,
        &context,
        "Current title",
        "Current findings",
        &report,
        Envelope::default(),
    ))
    .unwrap();
    let adopted = only_issue(&github);
    assert_eq!(adopted.number, old.number);
    assert_eq!(adopted.title, "Current title");
    assert!(
        adopted
            .body
            .contains(&marker::issue(&context.instance, IssueKind::Regression))
    );
    assert_eq!(
        marker::find_analyzed_sha(&adopted.body, &context.instance),
        Some(report.report.commit)
    );
    assert!(!adopted.body.contains(&old.body));
}

#[test]
fn adopted_issues_resume_normal_commit_order_guards_even_with_legacy_option_present() {
    let github = FakeGitHub::new();
    let context = context();
    legacy_issue(&github, &context, "Legacy report without a companion SHA.");
    let mut report = evidence(AnalysisMode::History, Outcome::Findings, true);
    block_on(publish_issue(
        &github,
        &context,
        "Renamed",
        "First findings",
        &report,
        Envelope::default(),
    ))
    .unwrap();
    let adopted = only_issue(&github);
    report.report.commit = sha('b');
    block_on(publish_issue(
        &github,
        &context,
        "Should not replace",
        "Unordered",
        &report,
        Envelope::default(),
    ))
    .unwrap();
    assert_eq!(only_issue(&github), adopted);
    github.set_comparison(&sha('a'), &sha('b'), Comparison { ahead_by: Some(1) });
    block_on(publish_issue(
        &github,
        &context,
        "Renamed again",
        "Forward findings",
        &report,
        Envelope::default(),
    ))
    .unwrap();
    assert_eq!(only_issue(&github).number, adopted.number);
    assert_eq!(only_issue(&github).title, "Renamed again");
}

#[test]
fn full_clean_history_can_adopt_and_close_a_legacy_regression() {
    let github = FakeGitHub::new();
    let context = context();
    let old = legacy_issue(&github, &context, "Unstructured legacy regression.");
    let report = evidence(AnalysisMode::History, Outcome::Clean, true);
    block_on(issue_cleanup(
        &github,
        &context,
        &report,
        true,
        Envelope::default(),
    ))
    .unwrap();
    assert!(github.issues().is_empty());
    let closed = github.closed_issues();
    assert_eq!(closed.len(), 1);
    let closed = closed.first().unwrap();
    assert_eq!(closed.number, old.number);
    assert!(
        closed
            .body
            .contains(&marker::issue(&context.instance, IssueKind::Regression))
    );
    assert_eq!(
        marker::find_analyzed_sha(&closed.body, &context.instance),
        Some(report.report.commit)
    );
}

#[test]
fn incomplete_clean_evidence_cannot_adopt_a_legacy_regression() {
    let github = FakeGitHub::new();
    let context = context();
    let old = legacy_issue(&github, &context, "Keep these findings.");
    let report = evidence(AnalysisMode::History, Outcome::Clean, false);
    let error = block_on(issue_cleanup(
        &github,
        &context,
        &report,
        true,
        Envelope::default(),
    ))
    .unwrap_err();
    assert!(error.find_source::<UnsafeAllClear>().is_some());
    assert_eq!(only_issue(&github), old);
    assert!(github.closed_issues().is_empty());
}

#[test]
fn ambiguous_title_adoption_does_not_mutate_or_create_issues() {
    let github = FakeGitHub::new();
    let context = context();
    legacy_issue(&github, &context, "First");
    legacy_issue(&github, &context, "Second");
    let before = github.issues();
    let report = evidence(AnalysisMode::History, Outcome::Findings, true);
    let error = block_on(publish_issue(
        &github,
        &context,
        "Current",
        "Findings",
        &report,
        Envelope::default(),
    ))
    .unwrap_err();
    assert!(error.find_source::<AmbiguousLegacyIssue>().is_some());
    assert_eq!(github.issues(), before);
}

#[test]
fn other_instance_ownership_survives_an_explicit_matching_legacy_title() {
    let github = FakeGitHub::new();
    let context = context();
    let identity = marker::issue(&"other".parse().unwrap(), IssueKind::Regression);
    let old = legacy_issue(&github, &context, &identity);
    let error = block_on(issue_preflight(&github, &context, &sha('a'))).unwrap_err();
    assert!(error.find_source::<AlreadyManagedLegacyIssue>().is_some());
    assert_eq!(only_issue(&github), old);
}

#[test]
fn alerts_adopt_the_legacy_failure_issue_and_resolve_the_same_identity() {
    let github = FakeGitHub::new();
    let context = context();
    let old = legacy_issue(&github, &context, "Old automation failure.");
    block_on(alert(
        &github,
        &context,
        "Current failure title",
        "https://example.test/failed",
        Envelope::default(),
    ))
    .unwrap();
    let adopted = only_issue(&github);
    assert_eq!(adopted.number, old.number);
    assert!(
        adopted
            .body
            .contains(&marker::issue(&context.instance, IssueKind::FailureAlert))
    );
    block_on(resolve_alert(
        &github,
        &context,
        "https://example.test/success",
    ))
    .unwrap();
    assert!(github.issues().is_empty());
    assert_eq!(github.closed_issues().first().unwrap().number, old.number);
}

#[test]
fn resolution_can_adopt_a_legacy_failure_without_an_intermediate_alert() {
    let github = FakeGitHub::new();
    let context = context();
    let old = legacy_issue(&github, &context, "Original failure details.");
    block_on(resolve_alert(
        &github,
        &context,
        "https://example.test/success",
    ))
    .unwrap();
    let closed = github.closed_issues();
    assert_eq!(closed.len(), 1);
    let closed = closed.first().unwrap();
    assert_eq!(closed.number, old.number);
    assert!(closed.body.contains(&old.body));
    assert!(
        closed
            .body
            .contains(&marker::issue(&context.instance, IssueKind::FailureAlert))
    );
    assert!(closed.body.contains("https://example.test/success"));
}

fn pr_context() -> Context {
    let mut context = context();
    context.comment_marker = Some("<!-- rolling-legacy -->".parse().unwrap());
    context.migration = MigrationOptions {
        issue_title: None,
        in_progress_marker: Some("<!-- collecting-legacy -->".parse().unwrap()),
    };
    context
}

#[test]
fn a_legacy_placeholder_becomes_owned_by_this_run_only() {
    let github = FakeGitHub::new();
    let context = pr_context();
    let head = sha('a');
    github.set_pull_head(1, head.clone());
    let original = block_on(github.create_comment(
        &context.repository,
        1,
        "<!-- rolling-legacy -->\n<!-- collecting-legacy -->\nLegacy placeholder",
    ))
    .unwrap();
    block_on(pr_comment_preflight(
        &github, &context, 1, "package", &head, 42,
    ))
    .unwrap();
    let comments = github.comments_for(1);
    assert_eq!(comments.len(), 1);
    let adopted = comments.first().unwrap();
    assert_eq!(adopted.id, original.id);
    assert!(
        adopted
            .body
            .contains(&marker::run_owner(&context.instance, 42, &head))
    );
    assert!(message::is_in_progress(&adopted.body, &context.instance));
    assert!(!adopted.body.contains("<!-- collecting-legacy -->"));
    block_on(pr_comment_finalize(
        &github,
        &context,
        1,
        "https://example.test/old",
        &head,
        41,
    ))
    .unwrap();
    assert_eq!(github.comments_for(1), comments);
    block_on(pr_comment_finalize(
        &github,
        &context,
        1,
        "https://example.test/current",
        &head,
        42,
    ))
    .unwrap();
    assert!(!message::is_in_progress(
        &github.comments_for(1).first().unwrap().body,
        &context.instance,
    ));
}

#[test]
fn legacy_placeholder_adoption_still_requires_the_frozen_live_head() {
    let github = FakeGitHub::new();
    let context = pr_context();
    github.set_pull_head(1, sha('b'));
    let original = block_on(github.create_comment(
        &context.repository,
        1,
        "<!-- rolling-legacy -->\n<!-- collecting-legacy -->",
    ))
    .unwrap();
    block_on(pr_comment_preflight(
        &github,
        &context,
        1,
        "package",
        &sha('a'),
        42,
    ))
    .unwrap();
    assert_eq!(github.comments_for(1), [original]);
}

#[test]
fn legacy_placeholder_recognition_is_exact_and_opt_in() {
    let mut options = MigrationOptions::default();
    assert!(!options.is_legacy_placeholder("<!-- collecting-legacy -->"));
    options.in_progress_marker = Some("<!-- collecting-legacy -->".parse().unwrap());
    assert!(options.is_legacy_placeholder("header\n<!-- collecting-legacy -->\nbody"));
    assert!(!options.is_legacy_placeholder("text <!-- collecting-legacy -->"));
    assert!(!options.is_legacy_placeholder("<!-- different-state -->"));
    for invalid in ["", " ", "Legacy\nTitle"] {
        let error = validate_legacy_title(invalid).unwrap_err();
        assert!(error.find_source::<InvalidLegacyIssueTitle>().is_some());
    }
}
