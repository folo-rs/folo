use std::any::type_name;
use std::collections::HashSet;
use std::num::NonZero;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::{env, fmt};

use ohno::AppError;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue, USER_AGENT};
use reqwest::{Method, Request, StatusCode, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::errors::{
    InvalidResponseError, MissingCreatedArtifactError, MissingTokenError, RequestFailedError,
    UnexpectedStatusError,
};
use crate::github::http::{Http, HttpResponse, RETRY_DELAYS, ReqwestHttp, retry_delay};
use crate::github::wire::{
    BodyWrite, CommentResponse, CompareResponse, IssueResponse, IssueStateWrite, IssueUpdate,
    IssueWrite, PullResponse, comparison,
};
use crate::github::{Comment, Comparison, GitHub, Issue, WorkflowJob};
use crate::model::{CommitSha, Repository};

/// GitHub request construction and policy over an injectable HTTP boundary.
pub(crate) struct RestGitHub<H = ReqwestHttp> {
    pub(crate) http: H,
    token: SecretToken,
    pub(crate) page_size: NonZero<usize>,
}

impl<H: fmt::Debug> fmt::Debug for RestGitHub<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("http", &self.http)
            .field("token", &self.token)
            .field("page_size", &self.page_size)
            .finish()
    }
}

impl RestGitHub {
    pub(crate) fn from_env() -> Result<Self, AppError> {
        let token = SecretToken::select(env::var("GITHUB_TOKEN").ok(), env::var("GH_TOKEN").ok())?;
        Ok(Self::new(ReqwestHttp::new()?, token, DEFAULT_PAGE_SIZE))
    }
}

impl<H: Http> RestGitHub<H> {
    pub(crate) fn new(http: H, token: SecretToken, page_size: NonZero<usize>) -> Self {
        Self {
            http,
            token,
            page_size,
        }
    }

    pub(crate) fn request(
        &self,
        method: Method,
        repository: &Repository,
        path: &str,
    ) -> Result<Request, AppError> {
        let operation = "building a GitHub request";
        let url = format!(
            "https://api.github.com/repos/{}/{}/{}",
            repository.owner(),
            repository.name(),
            path
        );
        let url =
            Url::parse(&url).map_err(|error| RequestFailedError::caused_by(operation, error))?;
        let mut request = Request::new(method, url);
        let mut authorization = HeaderValue::from_str(&format!("Bearer {}", self.token.expose()))
            .map_err(|error| RequestFailedError::caused_by(operation, error))?;
        authorization.set_sensitive(true);
        let headers = request.headers_mut();
        headers.insert(AUTHORIZATION, authorization);
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/vnd.github+json"),
        );
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static("cargo-bench-history-github"),
        );
        // Pin GitHub's documented REST representation rather than its changing default.
        headers.insert(
            "X-GitHub-Api-Version",
            HeaderValue::from_static("2022-11-28"),
        );
        Ok(request)
    }

    fn json_request(
        &self,
        method: Method,
        repository: &Repository,
        path: &str,
        body: &impl Serialize,
    ) -> Result<Request, AppError> {
        let mut request = self.request(method, repository, path)?;
        let body = serde_json::to_vec(body)
            .map_err(|error| RequestFailedError::caused_by("encoding a GitHub request", error))?;
        *request.body_mut() = Some(body.into());
        request
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        Ok(request)
    }

    pub(crate) async fn send_json<T: DeserializeOwned>(
        &self,
        operation: &str,
        request: Request,
    ) -> Result<T, AppError> {
        let response = self.send(operation, request).await?;
        decode(operation, &response)
    }

    async fn send_empty(&self, operation: &str, request: Request) -> Result<(), AppError> {
        _ = self.send(operation, request).await?;
        Ok(())
    }

    async fn send(&self, operation: &str, request: Request) -> Result<HttpResponse, AppError> {
        // Only these semantic operations are safe to repeat. In particular, a POST may
        // already have created its artifact even when the response was lost.
        let retry_safe = matches!(*request.method(), Method::GET | Method::PATCH);
        let mut delays = RETRY_DELAYS.into_iter();
        loop {
            let attempt = request
                .try_clone()
                .expect("GitHub requests contain only buffered, cloneable JSON bodies");
            let response = self.http.send(attempt).await;
            let fallback = retry_safe.then(|| delays.next()).flatten();
            match response {
                Ok(response) if response.status.is_success() => return Ok(response),
                Ok(response) => {
                    if let Some(delay) = fallback.and_then(|delay| retry_delay(&response, delay)) {
                        self.http.sleep(delay).await;
                        continue;
                    }
                    // A server error can echo input; credentials must not become diagnostics.
                    let body = String::from_utf8_lossy(&response.body)
                        .replace(self.token.expose(), "[REDACTED]");
                    return Err(UnexpectedStatusError::new(
                        operation,
                        response.status.as_u16(),
                        body,
                    )
                    .into());
                }
                Err(error) => {
                    if let Some(delay) = fallback.filter(|_| error.retryable()) {
                        self.http.sleep(delay).await;
                        continue;
                    }
                    return Err(RequestFailedError::caused_by(operation, error).into());
                }
            }
        }
    }

    async fn paginate<T: DeserializeOwned>(
        &self,
        operation: &str,
        request: Request,
        id: impl Fn(&T) -> NonZero<u64>,
    ) -> Result<Vec<T>, AppError> {
        const FIRST_PAGE: u64 = 1;

        let mut page = FIRST_PAGE;
        let mut result = Vec::new();
        let mut seen = HashSet::new();
        loop {
            let mut request = request
                .try_clone()
                .expect("listing requests have no body and are cloneable");
            request
                .url_mut()
                .query_pairs_mut()
                .append_pair("per_page", &self.page_size.to_string())
                .append_pair("page", &page.to_string());
            let values: Vec<T> = self.send_json(operation, request).await?;
            let last_page = values.len() < self.page_size.get();
            // A repeated page must fail, not loop or return an incomplete discovery set.
            if values.iter().any(|value| !seen.insert(id(value))) {
                return Err(PaginationError::new(operation).into());
            }
            result.extend(values);
            if last_page {
                return Ok(result);
            }
            page = page
                .checked_add(1)
                .ok_or_else(|| PaginationError::new(operation))?;
        }
    }
}

impl<H: Http> GitHub for RestGitHub<H> {
    async fn workflow_jobs(
        &self,
        repository: &Repository,
        run_id: NonZero<u64>,
    ) -> Result<Vec<WorkflowJob>, AppError> {
        self.list_jobs(repository, run_id).await
    }

    async fn open_issues(&self, repository: &Repository) -> Result<Vec<Issue>, AppError> {
        let mut request = self.request(Method::GET, repository, "issues")?;
        request
            .url_mut()
            .query_pairs_mut()
            .append_pair("state", "open");
        let values: Vec<IssueResponse> = self
            .paginate("listing open issues", request, |one: &IssueResponse| {
                one.number
            })
            .await?;
        Ok(values
            .into_iter()
            .filter(|one| one.pull_request.is_none())
            .map(Issue::from)
            .collect())
    }

    async fn create_issue(
        &self,
        repository: &Repository,
        title: &str,
        body: &str,
    ) -> Result<Issue, AppError> {
        let operation = "creating an issue";
        let request = self.json_request(
            Method::POST,
            repository,
            "issues",
            &IssueWrite { title, body },
        )?;
        let value: IssueResponse = self.send_json(operation, request).await?;
        if value.pull_request.is_some() {
            return Err(MissingCreatedArtifactError::new(operation).into());
        }
        Ok(value.into())
    }

    async fn update_issue(
        &self,
        repository: &Repository,
        number: u64,
        title: Option<&str>,
        body: &str,
    ) -> Result<(), AppError> {
        let number = artifact_id(number)?;
        let request = self.json_request(
            Method::PATCH,
            repository,
            &format!("issues/{number}"),
            &IssueUpdate { title, body },
        )?;
        self.send_empty("updating an issue", request).await
    }

    async fn close_issue(&self, repository: &Repository, number: u64) -> Result<(), AppError> {
        let number = artifact_id(number)?;
        let request = self.json_request(
            Method::PATCH,
            repository,
            &format!("issues/{number}"),
            &IssueStateWrite { state: "closed" },
        )?;
        self.send_empty("closing an issue", request).await
    }

    async fn comments(
        &self,
        repository: &Repository,
        pull_request: u64,
    ) -> Result<Vec<Comment>, AppError> {
        let pull_request = artifact_id(pull_request)?;
        let request = self.request(
            Method::GET,
            repository,
            &format!("issues/{pull_request}/comments"),
        )?;
        let values: Vec<CommentResponse> = self
            .paginate(
                "listing pull-request comments",
                request,
                |one: &CommentResponse| one.id,
            )
            .await?;
        Ok(values.into_iter().map(Comment::from).collect())
    }

    async fn create_comment(
        &self,
        repository: &Repository,
        pull_request: u64,
        body: &str,
    ) -> Result<Comment, AppError> {
        let pull_request = artifact_id(pull_request)?;
        let request = self.json_request(
            Method::POST,
            repository,
            &format!("issues/{pull_request}/comments"),
            &BodyWrite { body },
        )?;
        let value: CommentResponse = self
            .send_json("creating a pull-request comment", request)
            .await?;
        Ok(value.into())
    }

    async fn update_comment(
        &self,
        repository: &Repository,
        id: u64,
        body: &str,
    ) -> Result<(), AppError> {
        let id = artifact_id(id)?;
        let request = self.json_request(
            Method::PATCH,
            repository,
            &format!("issues/comments/{id}"),
            &BodyWrite { body },
        )?;
        self.send_empty("updating a pull-request comment", request)
            .await
    }

    async fn pull_request_head(
        &self,
        repository: &Repository,
        pull_request: u64,
    ) -> Result<CommitSha, AppError> {
        let pull_request = artifact_id(pull_request)?;
        let request = self.request(Method::GET, repository, &format!("pulls/{pull_request}"))?;
        let value: PullResponse = self
            .send_json("reading the pull-request head", request)
            .await?;
        value.head.sha.parse()
    }

    async fn compare(
        &self,
        repository: &Repository,
        base: &CommitSha,
        head: &CommitSha,
    ) -> Result<Comparison, AppError> {
        let operation = "comparing commits";
        let path = format!("compare/{}...{}", base.as_str(), head.as_str());
        let request = self.request(Method::GET, repository, &path)?;
        let value: CompareResponse = match self.send_json(operation, request).await {
            Ok(value) => value,
            Err(error) if is_not_found(&error) => return Ok(Comparison { ahead_by: None }),
            Err(error) => return Err(error),
        };
        comparison(&value)
    }
}

/// A credential whose diagnostic representation never reveals its contents.
pub(crate) struct SecretToken(String);

impl SecretToken {
    pub(crate) fn select(
        primary: Option<String>,
        fallback: Option<String>,
    ) -> Result<Self, AppError> {
        primary
            .filter(|value| !value.trim().is_empty())
            .or_else(|| fallback.filter(|value| !value.trim().is_empty()))
            .map(Self)
            .ok_or_else(|| MissingTokenError::new().into())
    }

    fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

/// Discovery cannot safely complete because GitHub pagination makes no progress.
#[ohno::error]
#[display("GitHub pagination did not advance while {operation}")]
pub(crate) struct PaginationError {
    operation: String,
}

// These errors expose no mutation, including through their stored error source.
impl UnwindSafe for PaginationError {}
impl RefUnwindSafe for PaginationError {}

/// A semantic operation was given an absent GitHub artifact identity.
#[ohno::error]
#[display("GitHub artifact identifiers must be nonzero")]
struct InvalidArtifactIdError;

impl UnwindSafe for InvalidArtifactIdError {}
impl RefUnwindSafe for InvalidArtifactIdError {}

// GitHub's largest supported page minimizes API calls without relying on Link URLs.
const DEFAULT_PAGE_SIZE: NonZero<usize> =
    NonZero::new(100).expect("GitHub's maximum page size is nonzero");

fn artifact_id(value: u64) -> Result<NonZero<u64>, AppError> {
    NonZero::new(value).ok_or_else(|| InvalidArtifactIdError::new().into())
}

fn decode<T: DeserializeOwned>(operation: &str, response: &HttpResponse) -> Result<T, AppError> {
    serde_json::from_slice(&response.body)
        .map_err(|error| InvalidResponseError::caused_by(operation, error).into())
}

fn is_not_found(error: &AppError) -> bool {
    error
        .find_source::<UnexpectedStatusError>()
        .is_some_and(|status| status.status() == StatusCode::NOT_FOUND.as_u16())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::RefCell;
    use std::collections::{BTreeMap, VecDeque};
    use std::future::{Future, ready};
    use std::io::{Error as IoError, ErrorKind};
    use std::time::Duration;

    use futures::executor::block_on;
    use reqwest::header::{HeaderMap, RETRY_AFTER};
    use serde_json::{Value, json};
    use static_assertions::assert_impl_all;

    use super::*;
    use crate::errors::InvalidCommitShaError;
    use crate::github::http::TransportError;
    use crate::message;
    use crate::operations::{Context, alert, pr_comment_preflight};

    assert_impl_all!(PaginationError: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(InvalidArtifactIdError: Send, Sync, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(TransportError: Send, Sync, UnwindSafe, RefUnwindSafe);

    /// Scripted exchanges and a recording sleeper, with no runtime or real clock.
    #[derive(Debug, Default)]
    struct FakeHttp {
        requests: RefCell<Vec<Request>>,
        responses: RefCell<VecDeque<Result<HttpResponse, TransportError>>>,
        delays: RefCell<Vec<Duration>>,
    }

    impl Http for FakeHttp {
        fn send(
            &self,
            request: Request,
        ) -> impl Future<Output = Result<HttpResponse, TransportError>> {
            self.requests.borrow_mut().push(request);
            // Exhaustion detects an unexpected attempt immediately, including under mutation testing.
            ready(self.responses.borrow_mut().pop_front().unwrap())
        }

        fn sleep(&self, delay: Duration) -> impl Future<Output = ()> {
            self.delays.borrow_mut().push(delay);
            ready(())
        }
    }

    /// A server that persists a serialized create but loses its acknowledgement.
    ///
    /// Discovery reads that stored representation through the real REST decoder. An optional
    /// discovery failure verifies that orchestration preserves the original transport source.
    #[derive(Debug)]
    struct CommittedCreateHttp {
        head: CommitSha,
        expected_requests: RefCell<VecDeque<(Method, &'static str)>>,
        artifact: RefCell<Option<Value>>,
        reconciliation_status: Option<StatusCode>,
        delays: RefCell<Vec<Duration>>,
    }

    impl Http for CommittedCreateHttp {
        fn send(
            &self,
            request: Request,
        ) -> impl Future<Output = Result<HttpResponse, TransportError>> {
            // Validate each lifecycle step as it executes, rather than after the operation
            // returns. An extra page cannot be satisfied forever by an empty fixture response.
            let (method, path) = self.expected_requests.borrow_mut().pop_front().unwrap();
            assert_eq!(request.method(), &method);
            assert_eq!(request.url().path(), path);
            let response = if request.method() == Method::POST {
                assert!(self.artifact.borrow().is_none());
                let body: Value =
                    serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
                let mut artifact = body;
                let id = if request.url().path().ends_with("/comments") {
                    "id"
                } else {
                    "number"
                };
                artifact
                    .as_object_mut()
                    .unwrap()
                    .insert(id.to_owned(), json!(21));
                *self.artifact.borrow_mut() = Some(artifact);
                Err(TransportError::caused_by(
                    true,
                    IoError::from(ErrorKind::ConnectionReset),
                ))
            } else {
                assert_eq!(request.method(), Method::GET);
                if request.url().path().ends_with("/pulls/17") {
                    Ok(response(
                        StatusCode::OK,
                        json!({
                            "number": 17, "head": {"sha": self.head.as_str()}
                        }),
                    ))
                } else if let Some(artifact) = self.artifact.borrow().as_ref() {
                    match self.reconciliation_status {
                        Some(status) => {
                            Ok(response(status, json!({"message": "Discovery denied"})))
                        }
                        None => Ok(response(StatusCode::OK, json!([artifact]))),
                    }
                } else {
                    Ok(response(StatusCode::OK, json!([])))
                }
            };
            ready(response)
        }

        fn sleep(&self, delay: Duration) -> impl Future<Output = ()> {
            self.delays.borrow_mut().push(delay);
            ready(())
        }
    }

    fn committed_create_github(
        reconciliation_status: Option<StatusCode>,
        expected_requests: impl IntoIterator<Item = (Method, &'static str)>,
    ) -> RestGitHub<CommittedCreateHttp> {
        RestGitHub::new(
            CommittedCreateHttp {
                head: "a".repeat(40).parse().unwrap(),
                expected_requests: RefCell::new(expected_requests.into_iter().collect()),
                artifact: RefCell::default(),
                reconciliation_status,
                delays: RefCell::default(),
            },
            SecretToken("test-bearer-credential".to_owned()),
            DEFAULT_PAGE_SIZE,
        )
    }

    fn context() -> Context {
        Context {
            repository: repository(),
            instance: "default".parse().unwrap(),
            verbose: false,
        }
    }

    fn github(responses: impl IntoIterator<Item = HttpResponse>) -> RestGitHub<FakeHttp> {
        github_at_page_size(responses, DEFAULT_PAGE_SIZE)
    }

    fn github_at_page_size(
        responses: impl IntoIterator<Item = HttpResponse>,
        page_size: NonZero<usize>,
    ) -> RestGitHub<FakeHttp> {
        RestGitHub::new(
            FakeHttp {
                responses: RefCell::new(responses.into_iter().map(Ok).collect()),
                ..FakeHttp::default()
            },
            SecretToken("test-bearer-credential".to_owned()),
            page_size,
        )
    }

    fn pagination_page_size() -> NonZero<usize> {
        // A full page must hold an issue and a pull request, followed by a shorter page
        // containing both null and empty bodies. Native coverage also crosses GitHub's
        // standard page boundary; Miri exercises the same runtime-configured algorithm.
        const INTERPRETER_PAGE_SIZE: usize = 3;
        if cfg!(miri) {
            NonZero::new(INTERPRETER_PAGE_SIZE).unwrap()
        } else {
            DEFAULT_PAGE_SIZE
        }
    }

    fn response(status: StatusCode, body: impl Serialize) -> HttpResponse {
        HttpResponse {
            status,
            headers: HeaderMap::new(),
            body: serde_json::to_vec(&body).unwrap(),
        }
    }

    fn raw_response(status: StatusCode, body: &[u8]) -> HttpResponse {
        HttpResponse {
            status,
            headers: HeaderMap::new(),
            body: body.to_vec(),
        }
    }

    fn repository() -> Repository {
        "folo-rs/folo".parse().unwrap()
    }

    fn issue(number: u64, body: impl Into<Value>) -> Value {
        let body = body.into();
        json!({
            "id": 42,
            "node_id": "I_kwDOexample",
            "url": format!("https://api.github.com/repos/folo-rs/folo/issues/{number}"),
            "number": number,
            "state": "open",
            "title": format!("Regression {number}"),
            "body": body,
            "labels": [],
            "user": {"login": "github-actions[bot]", "type": "Bot"}
        })
    }

    fn comment(id: u64, body: impl Into<Value>) -> Value {
        let body = body.into();
        json!({
            "id": id,
            "node_id": "IC_kwDOexample",
            "url": format!("https://api.github.com/repos/folo-rs/folo/issues/comments/{id}"),
            "body": body,
            "user": {"login": "github-actions[bot]", "type": "Bot"},
            "created_at": "2026-09-16T00:00:00Z"
        })
    }

    fn query(request: &Request) -> BTreeMap<String, String> {
        request.url().query_pairs().into_owned().collect()
    }

    fn assert_request(request: &Request, method: &Method, path: &str, body: Option<Value>) {
        assert_eq!(request.method(), method);
        assert_eq!(request.url().scheme(), "https");
        assert_eq!(request.url().host_str(), Some("api.github.com"));
        assert_eq!(request.url().path(), format!("/repos/folo-rs/folo/{path}"));
        assert_eq!(
            request.headers().get(ACCEPT).unwrap(),
            "application/vnd.github+json"
        );
        assert_eq!(
            request.headers().get(USER_AGENT).unwrap(),
            "cargo-bench-history-github"
        );
        assert_eq!(
            request.headers().get("X-GitHub-Api-Version").unwrap(),
            "2022-11-28"
        );
        let authorization = request.headers().get(AUTHORIZATION).unwrap();
        assert_eq!(authorization, "Bearer test-bearer-credential");
        assert!(authorization.is_sensitive());
        assert!(!format!("{request:?}").contains("test-bearer-credential"));
        match body {
            Some(body) => {
                assert_eq!(
                    request.headers().get(CONTENT_TYPE).unwrap(),
                    "application/json"
                );
                let actual: Value =
                    serde_json::from_slice(request.body().unwrap().as_bytes().unwrap()).unwrap();
                assert_eq!(actual, body);
            }
            None => assert!(request.body().is_none()),
        }
    }

    #[test]
    fn issue_pagination_includes_later_pages_and_skips_real_pull_request_objects() {
        let page_size = pagination_page_size();
        let last = u64::try_from(page_size.get()).unwrap();
        let mut first: Vec<_> = (1..=last)
            .map(|id| issue(id, json!("first page")))
            .collect();
        first.get_mut(0).unwrap().as_object_mut().unwrap().insert(
            "pull_request".to_owned(),
            json!({
                "url": "https://api.github.com/repos/folo-rs/folo/pulls/1",
                "html_url": "https://github.com/folo-rs/folo/pull/1",
                "diff_url": "https://github.com/folo-rs/folo/pull/1.diff",
                "patch_url": "https://github.com/folo-rs/folo/pull/1.patch"
            }),
        );
        let github = github_at_page_size(
            [
                response(StatusCode::OK, json!(first)),
                response(
                    StatusCode::OK,
                    json!([issue(last + 1, Value::Null), issue(last + 2, json!(""))]),
                ),
            ],
            page_size,
        );
        let issues = block_on(github.open_issues(&repository())).unwrap();
        assert!(issues.iter().map(|issue| issue.number).eq(2..=last + 2));
        assert_eq!(issues.first().unwrap().title, "Regression 2");
        assert_eq!(issues.first().unwrap().body, "first page");
        assert!(
            issues
                .iter()
                .rev()
                .take(2)
                .all(|issue| issue.body.is_empty())
        );
        let requests = github.http.requests.borrow();
        assert_eq!(requests.len(), 2);
        for (request, page) in requests.iter().zip(["1", "2"]) {
            assert_request(request, &Method::GET, "issues", None);
            assert_eq!(
                query(request),
                BTreeMap::from([
                    ("state".to_owned(), "open".to_owned()),
                    ("per_page".to_owned(), page_size.to_string()),
                    ("page".to_owned(), page.to_owned()),
                ])
            );
        }
        assert!(github.http.delays.borrow().is_empty());
    }

    #[test]
    fn comment_pagination_includes_later_pages_and_normalizes_empty_bodies() {
        let page_size = pagination_page_size();
        let last = u64::try_from(page_size.get()).unwrap();
        let first: Vec<_> = (1..=last).map(|id| comment(id, json!("report"))).collect();
        let github = github_at_page_size(
            [
                response(StatusCode::OK, json!(first)),
                response(
                    StatusCode::OK,
                    json!([comment(last + 1, Value::Null), comment(last + 2, json!(""))]),
                ),
            ],
            page_size,
        );
        let comments = block_on(github.comments(&repository(), 17)).unwrap();
        assert!(comments.iter().map(|comment| comment.id).eq(1..=last + 2));
        assert_eq!(comments.first().unwrap().body, "report");
        assert!(
            comments
                .iter()
                .rev()
                .take(2)
                .all(|comment| comment.body.is_empty())
        );
        let requests = github.http.requests.borrow();
        assert_eq!(requests.len(), 2);
        for (request, page) in requests.iter().zip(["1", "2"]) {
            assert_request(request, &Method::GET, "issues/17/comments", None);
            assert_eq!(
                query(request),
                BTreeMap::from([
                    ("per_page".to_owned(), page_size.to_string()),
                    ("page".to_owned(), page.to_owned()),
                ])
            );
        }
    }

    #[test]
    fn full_last_page_is_followed_by_an_empty_page() {
        let page_size = pagination_page_size();
        let last = u64::try_from(page_size.get()).unwrap();
        let first: Vec<_> = (1..=last).map(|id| comment(id, json!("report"))).collect();
        let github = github_at_page_size(
            [
                response(StatusCode::OK, json!(first)),
                response(StatusCode::OK, json!([])),
            ],
            page_size,
        );
        assert_eq!(
            block_on(github.comments(&repository(), 17)).unwrap().len(),
            page_size.get()
        );
        let requests = github.http.requests.borrow();
        assert_eq!(requests.len(), 2);
        for request in requests.iter() {
            assert_eq!(
                query(request).get("per_page").unwrap(),
                &page_size.to_string()
            );
        }
    }

    #[test]
    fn empty_lists_finish_after_one_page() {
        let github = github([
            response(StatusCode::OK, json!([])),
            response(StatusCode::OK, json!([])),
        ]);
        assert!(
            block_on(github.open_issues(&repository()))
                .unwrap()
                .is_empty()
        );
        assert!(
            block_on(github.comments(&repository(), 17))
                .unwrap()
                .is_empty()
        );
        let requests = github.http.requests.borrow();
        assert_eq!(requests.len(), 2);
        for request in requests.iter() {
            assert_eq!(query(request).get("per_page").unwrap(), "100");
        }
    }

    #[test]
    fn repeated_issue_pages_fail_instead_of_returning_partial_artifacts() {
        let page_size = pagination_page_size();
        let last = u64::try_from(page_size.get()).unwrap();
        let issues: Vec<_> = (1..=last).map(|id| issue(id, Value::Null)).collect();
        let github = github_at_page_size(
            [
                response(StatusCode::OK, json!(issues)),
                response(StatusCode::OK, json!(issues)),
            ],
            page_size,
        );
        assert!(
            block_on(github.open_issues(&repository()))
                .unwrap_err()
                .find_source::<PaginationError>()
                .is_some()
        );
        assert_eq!(github.http.requests.borrow().len(), 2);
    }

    #[test]
    fn repeated_comment_pages_fail_instead_of_returning_partial_artifacts() {
        let page_size = pagination_page_size();
        let last = u64::try_from(page_size.get()).unwrap();
        let comments: Vec<_> = (1..=last).map(|id| comment(id, Value::Null)).collect();
        let github = github_at_page_size(
            [
                response(StatusCode::OK, json!(comments)),
                response(StatusCode::OK, json!(comments)),
            ],
            page_size,
        );
        assert!(
            block_on(github.comments(&repository(), 17))
                .unwrap_err()
                .find_source::<PaginationError>()
                .is_some()
        );
        assert_eq!(github.http.requests.borrow().len(), 2);
    }

    #[test]
    fn later_page_errors_do_not_turn_into_successful_partial_lists() {
        let page_size = pagination_page_size();
        let last = u64::try_from(page_size.get()).unwrap();
        let first: Vec<_> = (1..=last).map(|id| issue(id, Value::Null)).collect();
        let github = github_at_page_size(
            [
                response(StatusCode::OK, json!(first)),
                response(
                    StatusCode::UNAUTHORIZED,
                    json!({"message": "Bad credentials"}),
                ),
            ],
            page_size,
        );
        let error = block_on(github.open_issues(&repository())).unwrap_err();
        assert_eq!(
            error
                .find_source::<UnexpectedStatusError>()
                .unwrap()
                .status(),
            401
        );
        assert_eq!(github.http.requests.borrow().len(), 2);
        assert!(github.http.delays.borrow().is_empty());
    }

    #[test]
    fn issue_write_endpoints_send_exact_methods_paths_and_json() {
        let body = "## Benchmark report\n\nUnicode: Δ; JSON escapes: \"quoted\".";
        let github = github([
            response(StatusCode::CREATED, issue(21, json!(body))),
            response(StatusCode::OK, issue(21, json!("updated"))),
            response(StatusCode::OK, issue(21, json!("updated"))),
            response(StatusCode::OK, issue(21, json!("updated"))),
        ]);
        block_on(async {
            let repo = repository();
            let issue = github
                .create_issue(&repo, "Benchmarks", body)
                .await
                .unwrap();
            assert_eq!(issue.number, 21);
            assert_eq!(issue.title, "Regression 21");
            assert_eq!(issue.body, body);
            github
                .update_issue(&repo, 21, Some("New title"), "updated")
                .await
                .unwrap();
            github
                .update_issue(&repo, 21, None, "updated")
                .await
                .unwrap();
            github.close_issue(&repo, 21).await.unwrap();
        });
        let requests = github.http.requests.borrow();
        let expected = [
            (
                Method::POST,
                "issues",
                Some(json!({"title": "Benchmarks", "body": body})),
            ),
            (
                Method::PATCH,
                "issues/21",
                Some(json!({"title": "New title", "body": "updated"})),
            ),
            (Method::PATCH, "issues/21", Some(json!({"body": "updated"}))),
            (Method::PATCH, "issues/21", Some(json!({"state": "closed"}))),
        ];
        assert_eq!(requests.len(), expected.len());
        for (request, (method, path, body)) in requests.iter().zip(expected) {
            assert_request(request, &method, path, body);
            assert!(query(request).is_empty());
        }
        assert!(github.http.delays.borrow().is_empty());
    }

    #[test]
    fn comment_write_endpoints_send_exact_methods_paths_and_json() {
        let body = "## Benchmark report\n\nUnicode: Δ; JSON escapes: \"quoted\".";
        let github = github([
            response(StatusCode::CREATED, comment(31, json!(body))),
            response(StatusCode::OK, comment(31, json!("updated"))),
        ]);
        block_on(async {
            let repo = repository();
            let comment = github.create_comment(&repo, 17, body).await.unwrap();
            assert_eq!(
                comment,
                Comment {
                    id: 31,
                    body: body.to_owned()
                }
            );
            github.update_comment(&repo, 31, "updated").await.unwrap();
        });
        let requests = github.http.requests.borrow();
        let expected = [
            (
                Method::POST,
                "issues/17/comments",
                Some(json!({"body": body})),
            ),
            (
                Method::PATCH,
                "issues/comments/31",
                Some(json!({"body": "updated"})),
            ),
        ];
        assert_eq!(requests.len(), expected.len());
        for (request, (method, path, body)) in requests.iter().zip(expected) {
            assert_request(request, &method, path, body);
            assert!(query(request).is_empty());
        }
        assert!(github.http.delays.borrow().is_empty());
    }

    #[test]
    fn pull_head_uses_real_nested_json_and_validates_the_sha() {
        let sha = "A".repeat(40);
        let github = github([
            response(
                StatusCode::OK,
                json!({
                    "number": 17, "state": "open",
                    "head": {"sha": sha, "ref": "feature", "repo": {"full_name": "folo-rs/folo"}}
                }),
            ),
            response(StatusCode::OK, json!({"head": {"sha": "not-a-commit"}})),
            response(StatusCode::OK, json!({"head": {}})),
        ]);
        assert_eq!(
            block_on(github.pull_request_head(&repository(), 17))
                .unwrap()
                .as_str(),
            "a".repeat(40)
        );
        assert!(
            block_on(github.pull_request_head(&repository(), 17))
                .unwrap_err()
                .find_source::<InvalidCommitShaError>()
                .is_some()
        );
        assert!(
            block_on(github.pull_request_head(&repository(), 17))
                .unwrap_err()
                .find_source::<InvalidResponseError>()
                .is_some()
        );
        for request in github.http.requests.borrow().iter() {
            assert_request(request, &Method::GET, "pulls/17", None);
        }
    }

    #[test]
    fn comparison_relationships_only_report_a_linear_forward_distance() {
        let base: CommitSha = "a".repeat(40).parse().unwrap();
        let head: CommitSha = "b".repeat(40).parse().unwrap();
        for (status, ahead_by, behind_by, expected) in [
            ("ahead", 7, 0, Some(7)),
            ("identical", 0, 0, Some(0)),
            ("behind", 0, 7, None),
            ("diverged", 3, 7, None),
        ] {
            let github = github([response(
                StatusCode::OK,
                json!({
                    "status": status,
                    "ahead_by": ahead_by,
                    "behind_by": behind_by,
                    "total_commits": ahead_by,
                    "merge_base_commit": {"sha": "c".repeat(40)},
                    "commits": []
                }),
            )]);
            assert_eq!(
                block_on(github.compare(&repository(), &base, &head)).unwrap(),
                Comparison { ahead_by: expected }
            );
            let requests = github.http.requests.borrow();
            assert_eq!(requests.len(), 1);
            assert_request(
                requests.first().unwrap(),
                &Method::GET,
                &format!("compare/{}...{}", base.as_str(), head.as_str()),
                None,
            );
        }
    }

    #[test]
    fn missing_comparison_is_unknown_but_authentication_failure_is_an_error() {
        let base = "a".repeat(40).parse().unwrap();
        let head = "b".repeat(40).parse().unwrap();
        let github = github([
            response(
                StatusCode::NOT_FOUND,
                json!({"message": "No common ancestor"}),
            ),
            response(
                StatusCode::UNAUTHORIZED,
                json!({"message": "Bad credentials"}),
            ),
        ]);
        assert_eq!(
            block_on(github.compare(&repository(), &base, &head))
                .unwrap()
                .ahead_by,
            None
        );
        assert!(
            block_on(github.compare(&repository(), &base, &head))
                .unwrap_err()
                .find_source::<UnexpectedStatusError>()
                .is_some()
        );
    }

    #[test]
    fn missing_and_unrecognized_comparison_statuses_are_invalid() {
        assert_invalid_comparisons([
            json!({"ahead_by": 0}),
            json!({"status": "unrelated", "ahead_by": 0, "behind_by": 0}),
        ]);
    }

    #[test]
    fn inconsistent_forward_comparison_distances_are_invalid() {
        assert_invalid_comparisons([
            json!({"status": "ahead", "ahead_by": 0, "behind_by": 0}),
            json!({"status": "ahead", "ahead_by": 1, "behind_by": 1}),
            json!({"status": "identical", "ahead_by": 1, "behind_by": 0}),
            json!({"status": "identical", "ahead_by": 0, "behind_by": 1}),
        ]);
    }

    #[test]
    fn inconsistent_reverse_comparison_distances_are_invalid() {
        assert_invalid_comparisons([
            json!({"status": "behind", "ahead_by": 0, "behind_by": 0}),
            json!({"status": "behind", "ahead_by": 1, "behind_by": 1}),
            json!({"status": "diverged", "ahead_by": 0, "behind_by": 1}),
            json!({"status": "diverged", "ahead_by": 1, "behind_by": 0}),
        ]);
    }

    fn assert_invalid_comparisons(bodies: impl IntoIterator<Item = Value>) {
        let base = "a".repeat(40).parse().unwrap();
        let head = "b".repeat(40).parse().unwrap();
        for body in bodies {
            let github = github([response(StatusCode::OK, body)]);
            assert!(
                block_on(github.compare(&repository(), &base, &head))
                    .unwrap_err()
                    .find_source::<InvalidResponseError>()
                    .is_some()
            );
        }
    }

    #[test]
    fn malformed_issue_list_responses_are_rejected_without_retry() {
        for body in [
            b"not json".as_slice(),
            b"{}",
            b"null",
            br#"[{"number":0,"title":"Invalid"}]"#,
            br#"[{"number":-1,"title":"Invalid"}]"#,
            br#"[{"number":"17","title":"Invalid"}]"#,
            br#"[{"title":"Missing identity"}]"#,
            br#"[{"number":17,"title":"Invalid body","body":42}]"#,
            br#"[{"number":18446744073709551616,"title":"Overflow"}]"#,
        ] {
            let github = github([raw_response(StatusCode::OK, body)]);
            let error = block_on(github.open_issues(&repository())).unwrap_err();
            assert!(error.find_source::<InvalidResponseError>().is_some());
            assert!(error.find_source::<serde_json::Error>().is_some());
            assert_eq!(github.http.requests.borrow().len(), 1);
            assert!(github.http.delays.borrow().is_empty());
        }
    }

    #[test]
    fn invalid_artifact_ids_are_rejected_without_retry() {
        for id in [json!(0), json!(-1), json!("17"), Value::Null] {
            let github = github([
                response(
                    StatusCode::CREATED,
                    json!({"number": id, "title": "Invalid"}),
                ),
                response(StatusCode::CREATED, json!({"id": id})),
                response(StatusCode::OK, json!([{"id": id}])),
            ]);
            assert!(
                block_on(github.create_issue(&repository(), "Title", "body"))
                    .unwrap_err()
                    .find_source::<InvalidResponseError>()
                    .is_some()
            );
            assert!(
                block_on(github.create_comment(&repository(), 17, "body"))
                    .unwrap_err()
                    .find_source::<InvalidResponseError>()
                    .is_some()
            );
            assert!(
                block_on(github.comments(&repository(), 17))
                    .unwrap_err()
                    .find_source::<InvalidResponseError>()
                    .is_some()
            );
            assert_eq!(github.http.requests.borrow().len(), 3);
        }
    }

    #[test]
    fn created_pull_request_object_is_not_an_issue_artifact() {
        let github = github([response(
            StatusCode::CREATED,
            json!({
                "number": 17, "title": "Unexpected", "body": null,
                "pull_request": {"url": "https://api.github.com/repos/folo-rs/folo/pulls/17"}
            }),
        )]);
        assert!(
            block_on(github.create_issue(&repository(), "Title", "body"))
                .unwrap_err()
                .find_source::<MissingCreatedArtifactError>()
                .is_some()
        );
    }

    #[test]
    fn zero_input_ids_never_reach_http() {
        let github = github([]);
        block_on(async {
            let repo = repository();
            let errors = [
                github.update_issue(&repo, 0, None, "").await.unwrap_err(),
                github.close_issue(&repo, 0).await.unwrap_err(),
                github.comments(&repo, 0).await.unwrap_err(),
                github.create_comment(&repo, 0, "").await.unwrap_err(),
                github.update_comment(&repo, 0, "").await.unwrap_err(),
                github.pull_request_head(&repo, 0).await.unwrap_err(),
            ];
            for error in errors {
                assert!(error.find_source::<InvalidArtifactIdError>().is_some());
            }
        });
        assert!(github.http.requests.borrow().is_empty());
    }

    #[test]
    fn transient_status_retries_are_bounded_and_delays_are_fake() {
        for status in [
            StatusCode::REQUEST_TIMEOUT,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            let github = github([
                response(status, json!({"message": "Try again"})),
                response(status, json!({"message": "Try again"})),
                response(status, json!({"message": "Try again"})),
            ]);
            let error = block_on(github.open_issues(&repository())).unwrap_err();
            assert_eq!(
                error
                    .find_source::<UnexpectedStatusError>()
                    .unwrap()
                    .status(),
                status.as_u16()
            );
            assert_eq!(github.http.requests.borrow().len(), 3);
            assert_eq!(
                *github.http.delays.borrow(),
                [Duration::from_millis(200), Duration::from_millis(800)]
            );
        }
    }

    #[test]
    fn successful_retry_resends_the_same_serialized_request() {
        let github = github([
            response(
                StatusCode::SERVICE_UNAVAILABLE,
                json!({"message": "Try again"}),
            ),
            response(StatusCode::OK, json!({})),
        ]);
        block_on(github.update_issue(&repository(), 21, Some("Title"), "body")).unwrap();
        let requests = github.http.requests.borrow();
        assert_eq!(requests.len(), 2);
        for request in requests.iter() {
            assert_request(
                request,
                &Method::PATCH,
                "issues/21",
                Some(json!({
                    "title": "Title", "body": "body"
                })),
            );
        }
        assert_eq!(*github.http.delays.borrow(), [Duration::from_millis(200)]);
    }

    #[test]
    fn retry_after_delta_seconds_are_honored_within_budget() {
        for status in [
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::FORBIDDEN,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            for (header, delay) in [("0", 0), ("7", 7), ("60", 60)] {
                let mut limited = response(status, json!({"message": "Rate limit exceeded"}));
                limited
                    .headers
                    .insert(RETRY_AFTER, HeaderValue::from_str(header).unwrap());
                let github = github([limited, response(StatusCode::OK, json!([]))]);
                block_on(github.open_issues(&repository())).unwrap();
                assert_eq!(github.http.requests.borrow().len(), 2);
                assert_eq!(*github.http.delays.borrow(), [Duration::from_secs(delay)]);
            }
        }
    }

    #[test]
    fn unsupported_retry_after_values_fail_without_retrying_early() {
        for header in [
            "61",
            "18446744073709551615",
            "-1",
            "invalid",
            "Wed, 16 Sep 2026 12:00:00 GMT",
        ] {
            let mut limited = response(StatusCode::TOO_MANY_REQUESTS, json!({}));
            limited
                .headers
                .insert(RETRY_AFTER, HeaderValue::from_str(header).unwrap());
            let github = github([limited]);
            assert!(
                block_on(github.open_issues(&repository()))
                    .unwrap_err()
                    .find_source::<UnexpectedStatusError>()
                    .is_some()
            );
            assert_eq!(github.http.requests.borrow().len(), 1);
            assert!(github.http.delays.borrow().is_empty());
        }
    }

    #[test]
    fn rate_limit_headers_distinguish_a_limit_from_ordinary_forbidden() {
        for status in [StatusCode::FORBIDDEN, StatusCode::TOO_MANY_REQUESTS] {
            let mut limited = response(status, json!({}));
            if status == StatusCode::FORBIDDEN {
                limited
                    .headers
                    .insert("x-ratelimit-remaining", HeaderValue::from_static("0"));
            }
            let github = github([limited, response(StatusCode::OK, json!([]))]);
            block_on(github.open_issues(&repository())).unwrap();
            assert_eq!(*github.http.delays.borrow(), [Duration::from_secs(60)]);
        }
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::UNPROCESSABLE_ENTITY,
        ] {
            let mut denied = response(status, json!({}));
            denied
                .headers
                .insert("x-ratelimit-remaining", HeaderValue::from_static("9"));
            let github = github([denied]);
            assert!(
                block_on(github.open_issues(&repository()))
                    .unwrap_err()
                    .find_source::<UnexpectedStatusError>()
                    .is_some()
            );
            assert!(github.http.delays.borrow().is_empty());
            assert_eq!(github.http.requests.borrow().len(), 1);
        }
    }

    #[test]
    fn creates_never_retry_http_errors_including_rate_limits_and_redirects() {
        for status in [
            StatusCode::TEMPORARY_REDIRECT,
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::REQUEST_TIMEOUT,
            StatusCode::UNPROCESSABLE_ENTITY,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::BAD_GATEWAY,
        ] {
            let github = github([response(status, json!({})), response(status, json!({}))]);
            assert!(
                block_on(github.create_issue(&repository(), "Title", "body"))
                    .unwrap_err()
                    .find_source::<UnexpectedStatusError>()
                    .is_some()
            );
            assert!(
                block_on(github.create_comment(&repository(), 17, "body"))
                    .unwrap_err()
                    .find_source::<UnexpectedStatusError>()
                    .is_some()
            );
            assert_eq!(github.http.requests.borrow().len(), 2);
            assert!(github.http.delays.borrow().is_empty());
        }
    }

    #[test]
    fn transport_failures_keep_sources_and_retry_only_safe_operations() {
        let github = github([]);
        github.http.responses.borrow_mut().extend([
            Err(TransportError::caused_by(
                true,
                IoError::from(ErrorKind::ConnectionReset),
            )),
            Err(TransportError::caused_by(
                true,
                IoError::from(ErrorKind::UnexpectedEof),
            )),
            Ok(response(StatusCode::OK, json!([]))),
        ]);
        assert!(
            block_on(github.open_issues(&repository()))
                .unwrap()
                .is_empty()
        );
        assert_eq!(github.http.requests.borrow().len(), 3);
        assert_eq!(
            *github.http.delays.borrow(),
            [Duration::from_millis(200), Duration::from_millis(800)]
        );

        for retryable in [true, false] {
            let github = self::github([]);
            github.http.responses.borrow_mut().extend([
                Err(TransportError::caused_by(
                    retryable,
                    IoError::from(ErrorKind::ConnectionReset),
                )),
                Err(TransportError::caused_by(
                    retryable,
                    IoError::from(ErrorKind::UnexpectedEof),
                )),
            ]);
            let errors = [
                block_on(github.create_issue(&repository(), "Title", "body")).unwrap_err(),
                block_on(github.create_comment(&repository(), 17, "body")).unwrap_err(),
            ];
            for (error, kind) in errors
                .iter()
                .zip([ErrorKind::ConnectionReset, ErrorKind::UnexpectedEof])
            {
                assert!(error.find_source::<RequestFailedError>().is_some());
                assert!(error.find_source::<TransportError>().is_some());
                assert_eq!(error.find_source::<IoError>().unwrap().kind(), kind);
                assert!(!format!("{error:?}").contains("test-bearer-credential"));
            }
            assert_eq!(github.http.requests.borrow().len(), 2);
            assert!(github.http.delays.borrow().is_empty());
        }
    }

    #[test]
    fn exhausted_transport_errors_and_unretryable_body_errors_remain_failures() {
        for retryable in [true, false] {
            let github = github([]);
            let attempts = if retryable { 3 } else { 1 };
            for _ in 0..attempts {
                github
                    .http
                    .responses
                    .borrow_mut()
                    .push_back(Err(TransportError::caused_by(
                        retryable,
                        IoError::from(ErrorKind::UnexpectedEof),
                    )));
            }
            let error = block_on(github.update_comment(&repository(), 31, "report")).unwrap_err();
            assert_eq!(
                error.find_source::<IoError>().unwrap().kind(),
                ErrorKind::UnexpectedEof
            );
            assert!(error.find_source::<RequestFailedError>().is_some());
            assert_eq!(github.http.requests.borrow().len(), attempts);
            assert_eq!(github.http.delays.borrow().len(), attempts - 1);
        }
    }

    #[test]
    fn credentials_are_redacted_and_empty_token_sources_fall_back() {
        let token =
            SecretToken::select(Some("primary".to_owned()), Some("fallback".to_owned())).unwrap();
        assert_eq!(token.expose(), "primary");
        assert!(!format!("{token:?}").contains("primary"));
        for primary in [None, Some(String::new()), Some(" \t".to_owned())] {
            assert_eq!(
                SecretToken::select(primary, Some("fallback".to_owned()))
                    .unwrap()
                    .expose(),
                "fallback"
            );
        }
        for (primary, fallback) in [
            (None, None),
            (Some(String::new()), None),
            (None, Some(" ".to_owned())),
        ] {
            assert!(
                SecretToken::select(primary, fallback)
                    .unwrap_err()
                    .find_source::<MissingTokenError>()
                    .is_some()
            );
        }
        let github = github([response(
            StatusCode::UNAUTHORIZED,
            json!({
                "message": "test-bearer-credential"
            }),
        )]);
        let error = block_on(github.open_issues(&repository())).unwrap_err();
        assert!(!format!("{error:?}").contains("test-bearer-credential"));
        assert!(!error.to_string().contains("test-bearer-credential"));
        // The scripted response is consumed, so Debug covers only the recorded sensitive header.
        assert!(!format!("{github:?}").contains("test-bearer-credential"));
    }

    #[test]
    fn invalid_authorization_is_a_request_error_without_an_http_attempt() {
        let mut github = github([]);
        github.token = SecretToken("credential\ninjected-header".to_owned());
        let error = block_on(github.open_issues(&repository())).unwrap_err();
        assert!(error.find_source::<RequestFailedError>().is_some());
        assert!(!format!("{error:?}").contains("credential"));
        assert!(github.http.requests.borrow().is_empty());
    }

    #[test]
    fn ambiguous_issue_create_reconciles_through_serialized_http() {
        for status in [None, Some(StatusCode::FORBIDDEN)] {
            let github = committed_create_github(
                status,
                [
                    (Method::GET, "/repos/folo-rs/folo/issues"),
                    (Method::POST, "/repos/folo-rs/folo/issues"),
                    (Method::GET, "/repos/folo-rs/folo/issues"),
                ],
            );
            let result = block_on(alert(
                &github,
                &context(),
                "https://github.com/folo-rs/folo/actions/runs/23",
            ));
            match status {
                None => result.unwrap(),
                Some(_) => {
                    let error = result.unwrap_err();
                    assert!(error.find_source::<RequestFailedError>().is_some());
                    assert!(error.find_source::<TransportError>().is_some());
                    assert_eq!(
                        error.find_source::<IoError>().unwrap().kind(),
                        ErrorKind::ConnectionReset
                    );
                }
            }
            assert!(github.http.expected_requests.borrow().is_empty());
            let artifact = github.http.artifact.borrow();
            let artifact = artifact.as_ref().unwrap();
            assert_eq!(artifact.get("title").unwrap(), message::FAILURE_TITLE);
            assert!(!artifact.get("body").unwrap().as_str().unwrap().is_empty());
            assert!(github.http.delays.borrow().is_empty());
        }
    }

    #[test]
    fn ambiguous_comment_create_reconciles_through_serialized_http() {
        for status in [None, Some(StatusCode::FORBIDDEN)] {
            let github = committed_create_github(
                status,
                [
                    (Method::GET, "/repos/folo-rs/folo/issues/17/comments"),
                    (Method::GET, "/repos/folo-rs/folo/pulls/17"),
                    (Method::POST, "/repos/folo-rs/folo/issues/17/comments"),
                    (Method::GET, "/repos/folo-rs/folo/issues/17/comments"),
                ],
            );
            let result = block_on(pr_comment_preflight(
                &github,
                &context(),
                17,
                "cpulist",
                &github.http.head,
                23,
            ));
            match status {
                None => result.unwrap(),
                Some(_) => {
                    let error = result.unwrap_err();
                    assert!(error.find_source::<RequestFailedError>().is_some());
                    assert!(error.find_source::<TransportError>().is_some());
                    assert_eq!(
                        error.find_source::<IoError>().unwrap().kind(),
                        ErrorKind::ConnectionReset
                    );
                }
            }
            assert!(github.http.expected_requests.borrow().is_empty());
            let artifact = github.http.artifact.borrow();
            assert!(
                !artifact
                    .as_ref()
                    .unwrap()
                    .get("body")
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .is_empty()
            );
            assert!(github.http.delays.borrow().is_empty());
        }
    }
}
