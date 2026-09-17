use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::future::{Future, ready};
use std::num::NonZero;

use ohno::AppError;

use crate::errors::{AmbiguousCreateError, RequestFailedError};
use crate::github::{Comment, Comparison, GitHub, Issue, WorkflowJob};
use crate::model::{CommitSha, Repository};

/// An in-memory GitHub port for orchestration tests.
#[derive(Debug, Default)]
pub(crate) struct FakeGitHub {
    issues: RefCell<BTreeMap<u64, Issue>>,
    closed_issues: RefCell<Vec<Issue>>,
    comments: RefCell<BTreeMap<u64, (u64, Comment)>>,
    pull_heads: RefCell<BTreeMap<u64, CommitSha>>,
    comparisons: RefCell<BTreeMap<(String, String), Comparison>>,
    next_id: Cell<u64>,
    fail_issue_create_after_commit: Cell<bool>,
    fail_comment_create_after_commit: Cell<bool>,
    fail_issue_list: Cell<bool>,
    fail_comment_list: Cell<bool>,
    issue_list_calls: Cell<usize>,
    comment_list_calls: Cell<usize>,
    fail_pull_head: Cell<bool>,
    jobs: RefCell<Vec<WorkflowJob>>,
}

impl FakeGitHub {
    pub(crate) fn set_jobs(&self, jobs: Vec<WorkflowJob>) {
        *self.jobs.borrow_mut() = jobs;
    }

    pub(crate) fn new() -> Self {
        Self {
            next_id: Cell::new(1),
            ..Self::default()
        }
    }

    pub(crate) fn issues(&self) -> Vec<Issue> {
        self.issues.borrow().values().cloned().collect()
    }

    pub(crate) fn closed_issues(&self) -> Vec<Issue> {
        self.closed_issues.borrow().clone()
    }

    pub(crate) fn comments_for(&self, pull_request: u64) -> Vec<Comment> {
        self.comments
            .borrow()
            .values()
            .filter(|(pr, _)| *pr == pull_request)
            .map(|(_, comment)| comment.clone())
            .collect()
    }

    pub(crate) fn set_pull_head(&self, pull_request: u64, sha: CommitSha) {
        self.pull_heads.borrow_mut().insert(pull_request, sha);
    }

    pub(crate) fn set_comparison(
        &self,
        base: &CommitSha,
        head: &CommitSha,
        comparison: Comparison,
    ) {
        self.comparisons.borrow_mut().insert(
            (base.as_str().to_owned(), head.as_str().to_owned()),
            comparison,
        );
    }

    pub(crate) fn fail_next_issue_create_after_commit(&self) {
        self.fail_issue_create_after_commit.set(true);
    }

    pub(crate) fn fail_next_comment_create_after_commit(&self) {
        self.fail_comment_create_after_commit.set(true);
    }

    pub(crate) fn fail_issue_list(&self) {
        self.fail_issue_list.set(true);
    }

    pub(crate) fn fail_comment_list(&self) {
        self.fail_comment_list.set(true);
    }

    pub(crate) fn fail_pull_head(&self) {
        self.fail_pull_head.set(true);
    }

    fn next_id(&self) -> u64 {
        let id = self.next_id.get();
        self.next_id.set(
            id.checked_add(1)
                .expect("the in-memory fake cannot create u64::MAX artifacts"),
        );
        id
    }
}

impl GitHub for FakeGitHub {
    fn workflow_jobs(
        &self,
        _repository: &Repository,
        _run_id: NonZero<u64>,
    ) -> impl Future<Output = Result<Vec<WorkflowJob>, AppError>> {
        ready(Ok(self.jobs.borrow().clone()))
    }

    fn open_issues(
        &self,
        _repository: &Repository,
    ) -> impl Future<Output = Result<Vec<Issue>, AppError>> {
        let calls = self.issue_list_calls.get();
        self.issue_list_calls.set(
            calls
                .checked_add(1)
                .expect("the fake cannot perform usize::MAX issue-list calls"),
        );
        if self.fail_issue_list.get() && calls != 0 {
            return ready(Err(RequestFailedError::new("listing fake issues").into()));
        }
        ready(Ok(self.issues()))
    }

    fn create_issue(
        &self,
        _repository: &Repository,
        _title: &str,
        body: &str,
    ) -> impl Future<Output = Result<Issue, AppError>> {
        let issue = Issue {
            number: self.next_id(),
            title: _title.to_owned(),
            body: body.to_owned(),
        };
        self.issues.borrow_mut().insert(issue.number, issue.clone());
        if self.fail_issue_create_after_commit.replace(false) {
            return ready(Err(AmbiguousCreateError::new().into()));
        }
        ready(Ok(issue))
    }

    fn update_issue(
        &self,
        _repository: &Repository,
        number: u64,
        title: Option<&str>,
        body: &str,
    ) -> impl Future<Output = Result<(), AppError>> {
        if let Some(issue) = self.issues.borrow_mut().get_mut(&number) {
            if let Some(title) = title {
                issue.title = title.to_owned();
            }
            issue.body = body.to_owned();
        }
        ready(Ok(()))
    }

    fn close_issue(
        &self,
        _repository: &Repository,
        number: u64,
    ) -> impl Future<Output = Result<(), AppError>> {
        if let Some(issue) = self.issues.borrow_mut().remove(&number) {
            self.closed_issues.borrow_mut().push(issue);
        }
        ready(Ok(()))
    }

    fn comments(
        &self,
        _repository: &Repository,
        pull_request: u64,
    ) -> impl Future<Output = Result<Vec<Comment>, AppError>> {
        let calls = self.comment_list_calls.get();
        self.comment_list_calls.set(
            calls
                .checked_add(1)
                .expect("the fake cannot perform usize::MAX comment-list calls"),
        );
        if self.fail_comment_list.get() && calls != 0 {
            return ready(Err(RequestFailedError::new("listing fake comments").into()));
        }
        ready(Ok(self.comments_for(pull_request)))
    }

    fn create_comment(
        &self,
        _repository: &Repository,
        pull_request: u64,
        body: &str,
    ) -> impl Future<Output = Result<Comment, AppError>> {
        let comment = Comment {
            id: self.next_id(),
            body: body.to_owned(),
        };
        self.comments
            .borrow_mut()
            .insert(comment.id, (pull_request, comment.clone()));
        if self.fail_comment_create_after_commit.replace(false) {
            return ready(Err(AmbiguousCreateError::new().into()));
        }
        ready(Ok(comment))
    }

    fn update_comment(
        &self,
        _repository: &Repository,
        id: u64,
        body: &str,
    ) -> impl Future<Output = Result<(), AppError>> {
        if let Some((_, comment)) = self.comments.borrow_mut().get_mut(&id) {
            comment.body = body.to_owned();
        }
        ready(Ok(()))
    }

    fn pull_request_head(
        &self,
        _repository: &Repository,
        pull_request: u64,
    ) -> impl Future<Output = Result<CommitSha, AppError>> {
        if self.fail_pull_head.get() {
            return ready(Err(RequestFailedError::new(
                "reading the fake pull-request head",
            )
            .into()));
        }
        ready(Ok(self
            .pull_heads
            .borrow()
            .get(&pull_request)
            .cloned()
            .expect("the test must seed the pull-request head")))
    }

    fn compare(
        &self,
        _repository: &Repository,
        base: &CommitSha,
        head: &CommitSha,
    ) -> impl Future<Output = Result<Comparison, AppError>> {
        ready(Ok(self
            .comparisons
            .borrow()
            .get(&(base.as_str().to_owned(), head.as_str().to_owned()))
            .copied()
            .unwrap_or(Comparison { ahead_by: None })))
    }
}
