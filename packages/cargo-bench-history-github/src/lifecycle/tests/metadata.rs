use futures::executor::block_on;

use crate::github::GitHub;
use crate::github::fake::FakeGitHub;
use crate::lifecycle::tests::harness::{context, findings, only_comment, owner, sha};
use crate::lifecycle::{IssueBody, UninterpretableIssue, annotate, comment_preflight};
use crate::model::IssueKind;
use crate::{marker, message};

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
