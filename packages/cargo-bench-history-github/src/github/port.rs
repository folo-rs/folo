use std::future::Future;
use std::num::NonZero;

use ohno::AppError;

use crate::github::WorkflowJob;
use crate::model::{CommitSha, Repository};

/// A directly read GitHub issue with its current title, state and body.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Issue {
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) open: bool,
}

/// Search metadata identifies candidates without trusting indexed issue contents.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct IssueCandidate {
    pub(crate) number: u64,
    pub(crate) title: String,
}

/// A discovered PR conversation body and its update identity for the rolling-comment lifecycle.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Comment {
    pub(crate) id: u64,
    pub(crate) body: String,
}

/// Verified forward ancestry used for replacement authority and staleness wording.
///
/// `None` carries no positive ordering proof; reverse, divergent and unavailable comparisons
/// must not be interpreted as an identical or fresh commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Comparison {
    pub(crate) ahead_by: Option<u64>,
}

/// Semantic GitHub operations used by publication and workflow-evidence orchestration.
///
/// Implementations provide complete discovery and direct artifact reads. The caller owns
/// lifecycle decisions and ambiguous-create reconciliation; no close/reopen operation is exposed.
pub(crate) trait GitHub {
    /// Supplies all attempts so a failed retry cannot inherit an older collection success.
    fn workflow_jobs(
        &self,
        repository: &Repository,
        run_id: NonZero<u64>,
    ) -> impl Future<Output = Result<Vec<WorkflowJob>, AppError>>;

    /// Finds issue-title candidates within the repository, optionally including closed alerts.
    fn search_issues(
        &self,
        repository: &Repository,
        phrase: &str,
        include_closed: bool,
    ) -> impl Future<Output = Result<Vec<IssueCandidate>, AppError>>;

    /// Obtains authoritative current contents after title discovery has selected a candidate.
    fn read_issue(
        &self,
        repository: &Repository,
        number: u64,
    ) -> impl Future<Output = Result<Issue, AppError>>;

    /// Attempts one creation whose ambiguous outcome the lifecycle must reconcile, not replay.
    fn create_issue(
        &self,
        repository: &Repository,
        title: &str,
        body: &str,
    ) -> impl Future<Output = Result<Issue, AppError>>;

    /// Updates title and body of a known issue without changing its open/closed disposition.
    fn update_issue(
        &self,
        repository: &Repository,
        number: u64,
        title: &str,
        body: &str,
    ) -> impl Future<Output = Result<(), AppError>>;

    /// Lists the PR conversation completely so marker discovery can detect absence or ambiguity.
    fn comments(
        &self,
        repository: &Repository,
        pull_request: u64,
    ) -> impl Future<Output = Result<Vec<Comment>, AppError>>;

    /// Attempts one PR-comment creation; the caller reconciles an uncertain result by identity.
    fn create_comment(
        &self,
        repository: &Repository,
        pull_request: u64,
        body: &str,
    ) -> impl Future<Output = Result<Comment, AppError>>;

    /// Replaces the body at a known comment identity after lifecycle ownership checks.
    fn update_comment(
        &self,
        repository: &Repository,
        id: u64,
        body: &str,
    ) -> impl Future<Output = Result<(), AppError>>;

    /// Reads the live head used by preflight and finish-side freshness guards.
    fn pull_request_head(
        &self,
        repository: &Repository,
        pull_request: u64,
    ) -> impl Future<Output = Result<CommitSha, AppError>>;

    /// Supplies verified directional commit evidence rather than a run-number ordering proxy.
    fn compare(
        &self,
        repository: &Repository,
        base: &CommitSha,
        head: &CommitSha,
    ) -> impl Future<Output = Result<Comparison, AppError>>;
}
