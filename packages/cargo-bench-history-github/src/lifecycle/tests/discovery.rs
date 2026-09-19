use futures::executor::block_on;

use crate::errors::AmbiguousCreateError;
use crate::github::GitHub;
use crate::github::fake::FakeGitHub;
use crate::identity::IssueIdentity;
use crate::lifecycle::tests::harness::{clock, context, findings, only_issue, owner, publish, sha};
use crate::lifecycle::{comment_preflight, issue_preflight, issue_report};
use crate::result::PublicationState;
use crate::{marker, message};

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
