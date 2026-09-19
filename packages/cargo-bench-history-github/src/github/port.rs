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

/// A GitHub pull-request comment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Comment {
    pub(crate) id: u64,
    pub(crate) body: String,
}

/// How far the compared head is ahead of the analyzed commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Comparison {
    pub(crate) ahead_by: Option<u64>,
}

/// Semantic GitHub operations used by the report lifecycles.
pub(crate) trait GitHub {
    fn workflow_jobs(
        &self,
        repository: &Repository,
        run_id: NonZero<u64>,
    ) -> impl Future<Output = Result<Vec<WorkflowJob>, AppError>>;

    fn search_issues(
        &self,
        repository: &Repository,
        phrase: &str,
        include_closed: bool,
    ) -> impl Future<Output = Result<Vec<IssueCandidate>, AppError>>;

    fn read_issue(
        &self,
        repository: &Repository,
        number: u64,
    ) -> impl Future<Output = Result<Issue, AppError>>;

    fn create_issue(
        &self,
        repository: &Repository,
        title: &str,
        body: &str,
    ) -> impl Future<Output = Result<Issue, AppError>>;

    fn update_issue(
        &self,
        repository: &Repository,
        number: u64,
        title: &str,
        body: &str,
    ) -> impl Future<Output = Result<(), AppError>>;

    fn comments(
        &self,
        repository: &Repository,
        pull_request: u64,
    ) -> impl Future<Output = Result<Vec<Comment>, AppError>>;

    fn create_comment(
        &self,
        repository: &Repository,
        pull_request: u64,
        body: &str,
    ) -> impl Future<Output = Result<Comment, AppError>>;

    fn update_comment(
        &self,
        repository: &Repository,
        id: u64,
        body: &str,
    ) -> impl Future<Output = Result<(), AppError>>;

    fn pull_request_head(
        &self,
        repository: &Repository,
        pull_request: u64,
    ) -> impl Future<Output = Result<CommitSha, AppError>>;

    fn compare(
        &self,
        repository: &Repository,
        base: &CommitSha,
        head: &CommitSha,
    ) -> impl Future<Output = Result<Comparison, AppError>>;
}
