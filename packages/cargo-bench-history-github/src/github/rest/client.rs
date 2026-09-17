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
    BodyWrite, CommentResponse, CompareResponse, IssueResponse, IssueUpdate, IssueWrite,
    PullResponse, comparison,
};
use crate::github::{Comment, Comparison, GitHub, Issue, IssueCandidate, WorkflowJob};
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
        serde_json::from_slice(&response.body)
            .map_err(|error| InvalidResponseError::caused_by(operation, error).into())
    }

    async fn send_empty(&self, operation: &str, request: Request) -> Result<(), AppError> {
        _ = self.send(operation, request).await?;
        Ok(())
    }

    async fn send(&self, operation: &str, request: Request) -> Result<HttpResponse, AppError> {
        // A POST may already have created its artifact even when the response was lost.
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
                    // Servers can echo input; credentials must not become diagnostics.
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
            if values.len() > self.page_size.get()
                || values.iter().any(|value| !seen.insert(id(value)))
            {
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

    async fn search_issues(
        &self,
        repository: &Repository,
        phrase: &str,
        include_closed: bool,
    ) -> Result<Vec<IssueCandidate>, AppError> {
        self.search(repository, phrase, include_closed).await
    }

    async fn read_issue(&self, repository: &Repository, number: u64) -> Result<Issue, AppError> {
        let number = artifact_id(number)?;
        let operation = "reading an issue";
        let request = self.request(Method::GET, repository, &format!("issues/{number}"))?;
        let value: IssueResponse = self.send_json(operation, request).await?;
        if value.number != number || value.pull_request.is_some() {
            return Err(InvalidResponseError::new(operation).into());
        }
        Ok(value.into())
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
        title: &str,
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

/// Discovery cannot safely complete because pagination did not make consistent progress.
#[ohno::error]
#[display("GitHub pagination did not advance while {operation}")]
pub(crate) struct PaginationError {
    operation: String,
}

/// A semantic operation was given an absent GitHub artifact identity.
#[ohno::error]
#[display("GitHub artifact identifiers must be nonzero")]
pub(crate) struct InvalidArtifactIdError;

// These errors expose no mutation, including through their stored error source.
impl UnwindSafe for PaginationError {}
impl RefUnwindSafe for PaginationError {}
impl UnwindSafe for InvalidArtifactIdError {}
impl RefUnwindSafe for InvalidArtifactIdError {}

// GitHub's largest supported page minimizes calls without relying on Link URLs.
const DEFAULT_PAGE_SIZE: NonZero<usize> =
    NonZero::new(100).expect("GitHub's maximum page size is nonzero");

fn artifact_id(value: u64) -> Result<NonZero<u64>, AppError> {
    NonZero::new(value).ok_or_else(|| InvalidArtifactIdError::new().into())
}

fn is_not_found(error: &AppError) -> bool {
    error
        .find_source::<UnexpectedStatusError>()
        .is_some_and(|status| status.status() == StatusCode::NOT_FOUND.as_u16())
}
