use std::any::type_name;
use std::collections::BTreeSet;
use std::env;
use std::fmt::{self, Debug, Formatter};
use std::time::Duration;

use ohno::AppError;
use reqwest::blocking::Client;
use reqwest::{Method, StatusCode};
use semver::Version;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::publication::binaries::model::Asset;
use crate::publication::context::WorkflowRun;
use crate::publication::manifest::{InvalidManifest, immutable_commit};

/// The GitHub REST boundary is independent of source/version policy.
pub struct Github {
    client: Client,
    repository: String,
    endpoint: String,
    token: Option<String>,
}

impl Github {
    #[cfg_attr(test, mutants::skip)] // Issue transport is exercised by a loopback boundary test.
    pub fn report_failure(&self, context: WorkflowRun, body: &str) -> Result<(), AppError> {
        let title = format!("Release failed: workflow run {}", context.run_id);
        let marker = format!("<!-- cargo-release-plan:{}:", context.run_id);
        let mut page = 1_usize;
        let issue = loop {
            let issues = self
                .get(&format!("/issues?state=all&per_page=100&page={page}"))?
                .ok_or_else(|| GithubResourceMissing::new("repository issues".to_owned()))?;
            let issues: Vec<Value> = serde_json::from_value(issues)?;
            if let Some(issue) = issues.iter().find(|issue| {
                issue.get("title").and_then(Value::as_str) == Some(&title)
                    && issue.get("pull_request").is_none()
                    && issue
                        .get("body")
                        .and_then(Value::as_str)
                        .is_some_and(|body| body.contains(&marker))
            }) {
                break Some(
                    issue
                        .get("number")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| GithubResourceMissing::new("issue number".to_owned()))?,
                );
            }
            if issues.len() < 100 {
                break None;
            }
            page = page
                .checked_add(1)
                .ok_or_else(|| InvalidManifest::new("issue pagination overflow".to_owned()))?;
        };
        let body = format!(
            "{body}\n<!-- cargo-release-plan:{}:{} -->",
            context.run_id, context.run_attempt
        );
        match issue {
            Some(number) => {
                self.request(
                    &Method::PATCH,
                    &format!("/issues/{number}"),
                    Some(&json!({"body":body,"state":"open"})),
                )?;
            }
            None => {
                self.request(
                    &Method::POST,
                    "/issues",
                    Some(&json!({"title":title,"body":body})),
                )?;
            }
        }
        Ok(())
    }

    #[cfg_attr(test, mutants::skip)] // Environment and HTTP setup are native integration boundaries.
    pub fn new(repository: &str) -> Result<Self, AppError> {
        Self::with_endpoint(
            "https://api.github.com",
            repository,
            env::var("GH_TOKEN")
                .ok()
                .or_else(|| env::var("GITHUB_TOKEN").ok()),
        )
    }

    /// Internal transport injection used by loopback integration tests.
    #[cfg_attr(test, mutants::skip)] // Real client setup and repository response validation.
    pub fn with_endpoint(
        endpoint: &str,
        repository: &str,
        token: Option<String>,
    ) -> Result<Self, AppError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .user_agent(concat!("cargo-release-plan/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(GithubTransportError::caused_by)?;
        let github = Self {
            client,
            repository: repository.to_owned(),
            endpoint: endpoint.to_owned(),
            token,
        };
        let identity: RepositoryIdentity = serde_json::from_value(
            github
                .get("")?
                .ok_or_else(|| GithubResourceMissing::new("repository".to_owned()))?,
        )?;
        if !identity.full_name.eq_ignore_ascii_case(repository) {
            return Err(InvalidManifest::new(
                "GitHub repository identity differs from publication configuration".to_owned(),
            )
            .into());
        }
        Ok(github)
    }

    #[cfg_attr(test, mutants::skip)] // Pass-through to the real HTTP boundary.
    pub(crate) fn get(&self, suffix: &str) -> Result<Option<Value>, AppError> {
        self.request(&Method::GET, suffix, None)
    }

    #[cfg_attr(test, mutants::skip)] // HTTP behavior is covered by loopback integration tests.
    fn request(
        &self,
        method: &Method,
        suffix: &str,
        body: Option<&Value>,
    ) -> Result<Option<Value>, AppError> {
        let url = format!("{}/repos/{}{}", self.endpoint, self.repository, suffix);
        let mut request = self
            .client
            .request(method.clone(), url)
            .header("Accept", "application/vnd.github+json");
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        } else if method != Method::GET {
            return Err(GithubTokenMissing::new().into());
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().map_err(GithubTransportError::caused_by)?;
        if response.status() == StatusCode::NOT_FOUND && method == Method::GET {
            return Ok(None);
        }
        let response = response
            .error_for_status()
            .map_err(GithubTransportError::caused_by)?;
        response
            .json()
            .map(Some)
            .map_err(|error| GithubTransportError::caused_by(error).into())
    }

    #[cfg_attr(test, mutants::skip)] // Resolves actual forge objects; orchestration uses the Forge fake.
    pub fn tag(&self, tag: &str) -> Result<Option<String>, AppError> {
        let Some(value) = self.get(&format!("/git/ref/tags/{tag}"))? else {
            return Ok(None);
        };
        let reference: GitReference = serde_json::from_value(value)?;
        let mut object = reference.object;
        let mut seen = BTreeSet::new();
        while object.kind == "tag" {
            if !seen.insert(object.sha.clone()) {
                return Err(InvalidManifest::new(
                    "GitHub tag contains a repeated object identity".to_owned(),
                )
                .into());
            }
            let value = self
                .get(&format!("/git/tags/{}", object.sha))?
                .ok_or_else(|| GithubResourceMissing::new(tag.to_owned()))?;
            object = serde_json::from_value::<GitReference>(value)?.object;
        }
        if object.kind != "commit" || !immutable_commit(&object.sha) {
            return Err(
                InvalidManifest::new("release tag must resolve to a commit".to_owned()).into(),
            );
        }
        Ok(Some(object.sha))
    }

    #[cfg_attr(test, mutants::skip)] // REST acquisition; the release guard has in-process tests.
    fn ensure_release(
        &self,
        tag: &str,
        version: &str,
        dry_run: bool,
    ) -> Result<Option<Release>, AppError> {
        if let Some(value) = self.get(&format!("/releases/tags/{tag}"))? {
            return Ok(Some(Release::parse(value, tag)?));
        }
        if dry_run {
            return Ok(None);
        }
        let result = self.request(
            &Method::POST,
            "/releases",
            Some(&json!({
                "tag_name": tag, "name": tag, "draft": false, "prerelease": !Version::parse(version)?.pre.is_empty()
            })),
        );
        if let Err(error) = result {
            if let Some(value) = self.get(&format!("/releases/tags/{tag}"))? {
                return Ok(Some(Release::parse(value, tag)?));
            }
            return Err(error);
        }
        Ok(Some(Release::parse(
            result?.ok_or_else(|| GithubResourceMissing::new(tag.to_owned()))?,
            tag,
        )?))
    }

    #[cfg_attr(test, mutants::skip)] // Native pagination is exercised with a full first-page fixture.
    fn assets(&self, release: &Release) -> Result<Vec<Asset>, AppError> {
        // GitHub's asset-list endpoint is paginated; unrelated manually uploaded assets
        // must not hide one of the archive/checksum pairs this process owns.
        const PAGE_SIZE: usize = 100;
        let mut assets = Vec::new();
        let mut page = 1_usize;
        loop {
            let value = self
                .get(&format!(
                    "/releases/{}/assets?per_page={PAGE_SIZE}&page={page}",
                    release.id
                ))?
                .ok_or_else(|| GithubResourceMissing::new("release assets".to_owned()))?;
            let entries: Vec<Asset> = serde_json::from_value(value)?;
            let complete = entries.len() < PAGE_SIZE;
            assets.extend(entries);
            if complete {
                return Ok(assets);
            }
            page = page.checked_add(1).ok_or_else(|| {
                InvalidManifest::new("release asset pagination overflow".to_owned())
            })?;
        }
    }
}

impl Debug for Github {
    #[cfg_attr(test, mutants::skip)] // Redaction is checked on the native client fixture.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("repository", &self.repository)
            .finish_non_exhaustive()
    }
}

impl Forge for Github {
    #[cfg_attr(test, mutants::skip)] // Native forwarding; the reconciliation sequence uses FakeForge.
    fn tag(&self, tag: &str) -> Result<Option<String>, AppError> {
        Self::tag(self, tag)
    }

    #[cfg_attr(test, mutants::skip)]
    fn create_tag(&self, tag: &str, source: &str) -> Result<(), AppError> {
        self.request(
            &Method::POST,
            "/git/refs",
            Some(&json!({
                "ref":format!("refs/tags/{tag}"),"sha":source
            })),
        )?;
        Ok(())
    }

    #[cfg_attr(test, mutants::skip)]
    fn ensure_release(
        &self,
        tag: &str,
        version: &str,
        dry_run: bool,
    ) -> Result<Option<Release>, AppError> {
        Self::ensure_release(self, tag, version, dry_run)
    }

    #[cfg_attr(test, mutants::skip)]
    fn assets(&self, release: &Release) -> Result<Vec<Asset>, AppError> {
        Self::assets(self, release)
    }
}

/// Supplies forge observations and writes to the in-process reconciliation sequence.
pub(crate) trait Forge {
    fn tag(&self, tag: &str) -> Result<Option<String>, AppError>;
    fn create_tag(&self, tag: &str, source: &str) -> Result<(), AppError>;
    fn ensure_release(
        &self,
        tag: &str,
        version: &str,
        dry_run: bool,
    ) -> Result<Option<Release>, AppError>;
    fn assets(&self, release: &Release) -> Result<Vec<Asset>, AppError>;
}

/// A forge release validated against the requested tag before its assets are inspected.
#[derive(Debug, Deserialize)]
pub(crate) struct Release {
    id: u64,
    tag_name: String,
    draft: bool,
}

impl Release {
    pub(crate) fn parse(value: Value, tag: &str) -> Result<Self, AppError> {
        let release: Self = serde_json::from_value(value)?;
        if release.tag_name != tag || release.draft {
            return Err(InvalidManifest::new(
                "GitHub release must be public and attached to the requested tag".to_owned(),
            )
            .into());
        }
        Ok(release)
    }
}

/// Confirms that the REST endpoint identifies the configured publication repository.
#[derive(Deserialize)]
struct RepositoryIdentity {
    full_name: String,
}

/// Carries the object reached through a tag reference or an annotated tag.
#[derive(Deserialize)]
struct GitReference {
    object: GitObject,
}

/// Identifies the next tag object to peel or the final commit to validate.
#[derive(Deserialize)]
struct GitObject {
    sha: String,
    #[serde(rename = "type")]
    kind: String,
}

#[ohno::error]
#[display("GitHub request could not complete")]
struct GithubTransportError;

#[ohno::error]
#[display("required GitHub resource is missing: {resource}")]
pub(crate) struct GithubResourceMissing {
    resource: String,
}

#[ohno::error]
#[display("GitHub writes require the invocation's GH_TOKEN or GITHUB_TOKEN")]
struct GithubTokenMissing;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn existing_release_must_be_public_and_attached_to_the_requested_tag() {
        Release::parse(
            json!({"id":1,"tag_name":"expected","draft":false}),
            "expected",
        )
        .unwrap();
        for (tag, draft) in [
            ("different", false),
            ("expected", true),
            ("different", true),
        ] {
            Release::parse(json!({"id":1,"tag_name":tag,"draft":draft}), "expected").unwrap_err();
        }
    }
}
