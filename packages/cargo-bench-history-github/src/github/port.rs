use std::future::Future;

use ohno::AppError;

use crate::model::{CommitSha, Repository};

/// A rolling GitHub issue.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Issue {
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) body: String,
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
    fn open_issues(
        &self,
        repository: &Repository,
    ) -> impl Future<Output = Result<Vec<Issue>, AppError>>;

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
        title: Option<&str>,
        body: &str,
    ) -> impl Future<Output = Result<(), AppError>>;

    fn close_issue(
        &self,
        repository: &Repository,
        number: u64,
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

    fn delete_comment(
        &self,
        repository: &Repository,
        id: u64,
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
