//! Shared in-memory contexts, frozen clocks and report fixtures for lifecycle tests.

use std::num::NonZero;
use std::time::{Duration, SystemTime};

use futures::executor::block_on;
use tick::Clock;

use crate::cli::{Conclusion, FailedArgs, PendingArgs, RunArgs};
use crate::github::fake::FakeGitHub;
use crate::github::{Comment, Issue};
use crate::identity::IssueIdentity;
use crate::lifecycle::{Report, issue_report};
use crate::marker;
use crate::model::{CommitSha, IssueKind};
use crate::operations::Context;
use crate::result::tests::evidence;
use crate::result::{AnalysisMode, Outcome};

pub(crate) fn context() -> Context {
    Context {
        repository: "folo-rs/folo".parse().unwrap(),
        instance: "project".parse().unwrap(),
        verbose: false,
    }
}

pub(crate) fn clock(day: u64) -> Clock {
    // Days relative to the epoch make update/no-op tests independent of calendar formatting.
    Clock::new_frozen_at(
        SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(day.checked_mul(86400).unwrap()))
            .unwrap(),
    )
}

pub(crate) fn sha(value: char) -> CommitSha {
    value.to_string().repeat(40).parse().unwrap()
}

pub(crate) fn owner(run: u64, attempt: u64, head: char) -> PendingArgs {
    PendingArgs {
        run: RunArgs {
            run_id: NonZero::new(run).unwrap(),
            run_attempt: NonZero::new(attempt).unwrap(),
        },
        head: sha(head),
    }
}

pub(crate) fn report(
    mode: AnalysisMode,
    outcome: Outcome,
    complete: bool,
    owner: PendingArgs,
) -> Report {
    let mut evidence = evidence(mode, outcome, complete);
    evidence.report.commit = owner.head.clone();
    Report {
        owner,
        evidence,
        summary: "  Tool-rendered summary.\n\n".to_owned(),
        artifact_url: Some("https://example.test/artifact".to_owned()),
    }
}

pub(crate) fn findings(head: char) -> Report {
    report(
        AnalysisMode::History,
        Outcome::Findings,
        true,
        owner(1, 1, head),
    )
}

pub(crate) fn failure(owner: PendingArgs, conclusion: Conclusion) -> FailedArgs {
    FailedArgs {
        run_url: format!(
            "https://github.com/folo-rs/folo/actions/runs/{}",
            owner.run.run_id
        ),
        pending: owner,
        conclusion,
    }
}

pub(crate) fn only_issue(github: &FakeGitHub) -> Issue {
    let issues = github.issues();
    assert_eq!(issues.len(), 1);
    issues.into_iter().next().unwrap()
}

pub(crate) fn only_comment(github: &FakeGitHub) -> Comment {
    let comments = github.comments_for(7);
    assert_eq!(comments.len(), 1);
    comments.into_iter().next().unwrap()
}

pub(crate) fn publish(github: &FakeGitHub, report: &Report, day: u64) {
    block_on(issue_report(
        github,
        &context(),
        &clock(day),
        report,
        report.evidence.publication_state(),
    ))
    .unwrap();
}

pub(crate) fn seed_retained_findings(github: &FakeGitHub, owner: &PendingArgs) {
    let context = context();
    // Annotation tests need coherent retained metadata, not another full publication lifecycle.
    // Keep interpreter setup small; report composition has its own coverage.
    let body = [
        marker::issue(&context.instance, IssueKind::Regression),
        marker::run_owner(&context.instance, owner),
        marker::analyzed_sha(&context.instance, &owner.head),
        marker::state(&context.instance, "findings"),
        "Retained findings".to_owned(),
    ]
    .join("\n");
    github.seed_issue(Issue {
        number: 1,
        title: IssueIdentity::Rolling(context.instance)
            .title(&clock(1))
            .unwrap(),
        body,
        open: true,
    });
}
