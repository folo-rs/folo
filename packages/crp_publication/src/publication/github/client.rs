use std::any::type_name;
use std::collections::BTreeSet;
use std::env;
use std::fmt::{self, Debug, Formatter};
use std::time::Duration;

use crp_workspace::identity::immutable_commit;
use ohno::AppError;
use reqwest::blocking::Client;
use reqwest::{Method, StatusCode};
use semver::Version;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::PublicationOutput;
use crate::publication::binaries::model::Asset;
use crate::publication::context::WorkflowRun;
use crate::publication::manifest::InvalidManifest;

/// The GitHub REST boundary is independent of source/version policy.
pub struct Github {
    client: Client,
    repository: String,
    endpoint: String,
    token: Option<String>,
}

impl Github {
    // The run is the issue identity; attempts replace and reopen its latest recovery handoff.
    #[cfg_attr(test, mutants::skip)] // Issue transport is exercised by a loopback boundary test.
    pub fn report_failure(&self, context: WorkflowRun, body: &str) -> Result<(), AppError> {
        // Bound discovery to a small part of the REST quota. Exceeding the budget is an error,
        // never permission to create a duplicate report after incomplete discovery.
        const MAX_PAGES: usize = 10;
        let title = format!("Release failed: workflow run {}", context.run_id);
        let marker = format!("<!-- cargo-release-plan:{}:", context.run_id);
        let creator = self.report_creator()?;
        let mut issue = None;
        for page in 1..=MAX_PAGES {
            let issues = self
                .get(&format!(
                    "/issues?state=all&creator={creator}&per_page={PAGE_SIZE}&page={page}"
                ))?
                .ok_or_else(|| GithubResourceMissing::new("repository issues".to_owned()))?;
            let issues: Vec<Value> = decode(issues, &format!("failure issue page {page}"))?;
            for candidate in &issues {
                if let Some(number) = report_issue(candidate, &creator, &title, &marker)?
                    && issue.replace(number).is_some()
                {
                    return Err(AmbiguousFailureReport::new(context.run_id.get()).into());
                }
            }
            if issues.len() < PAGE_SIZE {
                break;
            }
            if page == MAX_PAGES {
                return Err(FailureReportSearchIncomplete::new().into());
            }
        }
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

    #[cfg_attr(test, mutants::skip)] // Token ownership is acquired from the authenticated API.
    fn report_creator(&self) -> Result<String, AppError> {
        let token = self.token.as_ref().ok_or_else(GithubTokenMissing::new)?;
        let response = self
            .client
            .get(format!("{}/user", self.endpoint))
            .bearer_auth(token)
            .send()
            .map_err(GithubTransportError::caused_by)?;
        // Installation tokens have no user endpoint. Only the provider-owned Actions bot is
        // eligible in that case; arbitrary issue text or another bot cannot claim ownership.
        if response.status() == StatusCode::FORBIDDEN {
            return Ok("github-actions[bot]".to_owned());
        }
        let value = response
            .error_for_status()
            .map_err(GithubTransportError::caused_by)?
            .json()
            .map_err(GithubTransportError::caused_by)?;
        let identity: ReportCreator = decode(value, "authenticated issue creator")?;
        Ok(identity.login)
    }

    #[cfg_attr(test, mutants::skip)] // Environment and HTTP setup are native integration boundaries.
    pub fn new(repository: &str, output: &PublicationOutput) -> Result<Self, AppError> {
        Self::with_endpoint(
            "https://api.github.com",
            repository,
            select_token(env::var("GH_TOKEN").ok(), env::var("GITHUB_TOKEN").ok()),
            output,
        )
    }

    /// Internal transport injection used by loopback integration tests.
    #[cfg_attr(test, mutants::skip)] // Real client setup and repository response validation.
    pub fn with_endpoint(
        endpoint: &str,
        repository: &str,
        token: Option<String>,
        output: &PublicationOutput,
    ) -> Result<Self, AppError> {
        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(output.user_agent())
            .build()
            .map_err(GithubTransportError::caused_by)?;
        let github = Self {
            client,
            repository: repository.to_owned(),
            endpoint: endpoint.to_owned(),
            token: token.filter(|value| !value.trim().is_empty()),
        };
        let identity: RepositoryIdentity = decode(
            github
                .get("")?
                .ok_or_else(|| GithubResourceMissing::new("repository".to_owned()))?,
            "configured repository",
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
        let response = request.send().map_err(|error| {
            GithubRequestError::caused_by(method.to_string(), suffix.to_owned(), error)
        })?;
        if response.status() == StatusCode::NOT_FOUND && method == Method::GET {
            return Ok(None);
        }
        let response = response.error_for_status().map_err(|error| {
            GithubRequestError::caused_by(method.to_string(), suffix.to_owned(), error)
        })?;
        response.json().map(Some).map_err(|error| {
            GithubRequestError::caused_by(method.to_string(), suffix.to_owned(), error).into()
        })
    }

    #[cfg_attr(test, mutants::skip)] // Resolves actual forge objects; orchestration uses the Forge fake.
    pub fn tag(&self, tag: &str) -> Result<Option<String>, AppError> {
        let Some(value) = self.get(&format!("/git/ref/tags/{tag}"))? else {
            return Ok(None);
        };
        peel_tag(tag, value, |suffix| self.get(suffix)).map(Some)
    }

    #[cfg_attr(test, mutants::skip)] // REST acquisition; the release guard has in-process tests.
    fn ensure_release(
        &self,
        tag: &str,
        version: &str,
        source: &str,
        dry_run: bool,
    ) -> Result<Option<Release>, AppError> {
        if let Some(value) = self.get(&format!("/releases/tags/{tag}"))? {
            return Ok(Some(Release::parse(value, tag, version)?));
        }
        if dry_run {
            return Ok(None);
        }
        // If the established tag disappears, the API must not choose a moving default-branch
        // commit as an implicit replacement source.
        let result = self.request(
            &Method::POST,
            "/releases",
            Some(&json!({
                "tag_name": tag, "target_commitish": source, "name": tag, "draft": false,
                "prerelease": !Version::parse(version)?.pre.is_empty()
            })),
        );
        if let Err(error) = result {
            if let Some(value) = self.get(&format!("/releases/tags/{tag}"))? {
                return Ok(Some(Release::parse(value, tag, version)?));
            }
            return Err(error);
        }
        Ok(Some(Release::parse(
            result?.ok_or_else(|| GithubResourceMissing::new(tag.to_owned()))?,
            tag,
            version,
        )?))
    }

    #[cfg_attr(test, mutants::skip)] // Native pagination is exercised with a full first-page fixture.
    fn assets(&self, release: &Release) -> Result<Vec<Asset>, AppError> {
        // GitHub's asset-list endpoint is paginated; unrelated manually uploaded assets
        // must not hide one of the archive/checksum pairs this process owns.
        let mut assets = Vec::new();
        let mut page = 1_usize;
        loop {
            let value = self
                .get(&format!(
                    "/releases/{}/assets?per_page={PAGE_SIZE}&page={page}",
                    release.id
                ))?
                .ok_or_else(|| GithubResourceMissing::new("release assets".to_owned()))?;
            let entries: Vec<Asset> =
                decode(value, &format!("release {} asset page {page}", release.id))?;
            let complete = entries.len() < PAGE_SIZE;
            assets.extend(entries);
            if complete {
                return Ok(assets);
            }
            page = page
                .checked_add(1)
                .ok_or_else(GithubPaginationOverflow::new)?;
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
        source: &str,
        dry_run: bool,
    ) -> Result<Option<Release>, AppError> {
        Self::ensure_release(self, tag, version, source, dry_run)
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
        source: &str,
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
    prerelease: bool,
}

impl Release {
    pub(crate) fn parse(value: Value, tag: &str, version: &str) -> Result<Self, AppError> {
        let release: Self = decode(value, &format!("release for {tag}"))?;
        if release.tag_name != tag
            || release.draft
            || release.prerelease != !Version::parse(version)?.pre.is_empty()
        {
            return Err(ReleaseMismatch::new(tag.to_owned(), version.to_owned()).into());
        }
        Ok(release)
    }
}

/// Resolves acquired lightweight or annotated tag evidence identically for both forge adapters.
pub(crate) fn peel_tag(
    tag: &str,
    reference: Value,
    mut get: impl FnMut(&str) -> Result<Option<Value>, AppError>,
) -> Result<String, AppError> {
    let reference: GitReference = decode(reference, &format!("tag reference {tag}"))?;
    let mut object = reference.object;
    let mut seen = BTreeSet::new();
    while object.kind == "tag" {
        if !immutable_commit(&object.sha) || !seen.insert(object.sha.clone()) {
            return Err(
                InvalidTagIdentity::new(tag.to_owned(), "invalid or repeated tag object").into(),
            );
        }
        let value = get(&format!("/git/tags/{}", object.sha))?
            .ok_or_else(|| GithubResourceMissing::new(tag.to_owned()))?;
        object =
            decode::<GitReference>(value, &format!("annotated tag object {}", object.sha))?.object;
    }
    if object.kind != "commit" || !immutable_commit(&object.sha) {
        return Err(
            InvalidTagIdentity::new(tag.to_owned(), "expected a full commit identity").into(),
        );
    }
    Ok(object.sha)
}

// Use the REST maximum to minimize requests; full pages require a subsequent observation.
const PAGE_SIZE: usize = 100;
// An operational allowance for control-plane reads and writes, not native asset uploads.
// Slow requests fail with enough job time left for retained diagnostics and an explicit retry.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

fn select_token(primary: Option<String>, fallback: Option<String>) -> Option<String> {
    primary
        .filter(|value| !value.trim().is_empty())
        .or_else(|| fallback.filter(|value| !value.trim().is_empty()))
}

fn decode<T: DeserializeOwned>(value: Value, resource: &str) -> Result<T, AppError> {
    serde_json::from_value(value)
        .map_err(|error| GithubDecodeError::caused_by(resource.to_owned(), error).into())
}

fn report_issue(
    value: &Value,
    creator: &str,
    title: &str,
    marker: &str,
) -> Result<Option<u64>, AppError> {
    if value.get("title").and_then(Value::as_str) != Some(title)
        || value.get("pull_request").is_some()
        || value.pointer("/user/login").and_then(Value::as_str) != Some(creator)
        || !value
            .get("body")
            .and_then(Value::as_str)
            .is_some_and(|body| body.contains(marker))
    {
        return Ok(None);
    }
    value
        .get("number")
        .and_then(Value::as_u64)
        .map(Some)
        .ok_or_else(|| GithubResourceMissing::new("failure issue number".to_owned()).into())
}

/// The API-controlled owner of reports written with a user credential.
#[derive(Deserialize)]
struct ReportCreator {
    login: String,
}

#[ohno::error]
#[display("GitHub release for {tag} must be public and match version {version} prerelease status")]
struct ReleaseMismatch {
    tag: String,
    version: String,
}

#[ohno::error]
#[display("cannot decode GitHub resource {resource}")]
struct GithubDecodeError {
    resource: String,
}

/// Carries safe operation identity while the original transport error remains its cause.
#[ohno::error]
#[display("GitHub {method} {resource} could not complete")]
struct GithubRequestError {
    method: String,
    resource: String,
}

/// Remote ref evidence is not a valid peeled release commit.
#[ohno::error]
#[display("cannot verify GitHub tag {tag}: {reason}")]
struct InvalidTagIdentity {
    tag: String,
    reason: &'static str,
}

#[ohno::error]
#[display("GitHub release asset pagination overflow")]
struct GithubPaginationOverflow;

#[ohno::error]
#[display("multiple owned failure issues identify workflow run {run}")]
struct AmbiguousFailureReport {
    run: u64,
}

#[ohno::error]
#[display("failure issue discovery exceeded its request budget; no issue was changed")]
struct FailureReportSearchIncomplete;

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
    fn tag_peeling_requires_a_commit_and_rejects_cycles_and_missing_objects() {
        let direct = json!({"object":{"type":"commit","sha":"a".repeat(40)}});
        assert_eq!(
            peel_tag("tag", direct, |_| panic!("a commit needs no lookup")).unwrap(),
            "a".repeat(40)
        );
        let annotated = json!({"object":{"type":"tag","sha":"b".repeat(40)}});
        let mut lookups = Vec::new();
        let commit = peel_tag("tag", annotated.clone(), |suffix| {
            lookups.push(suffix.to_owned());
            Ok(Some(
                json!({"object":{"type":"commit","sha":"a".repeat(40)}}),
            ))
        })
        .unwrap();
        assert_eq!(commit, "a".repeat(40));
        assert_eq!(lookups, [format!("/git/tags/{}", "b".repeat(40))]);
        peel_tag("tag", annotated.clone(), |_| Ok(None)).unwrap_err();
        peel_tag("tag", annotated.clone(), |_| Ok(Some(annotated.clone()))).unwrap_err();
        for (kind, sha) in [
            ("tree", "a".repeat(40)),
            ("commit", "short".to_owned()),
            ("tag", "short".to_owned()),
        ] {
            peel_tag("tag", json!({"object":{"type":kind,"sha":sha}}), |_| {
                panic!("invalid objects need no lookup")
            })
            .unwrap_err();
        }
    }

    #[test]
    fn existing_release_must_be_public_and_attached_to_the_requested_tag() {
        Release::parse(
            json!({"id":1,"tag_name":"expected","draft":false,"prerelease":false}),
            "expected",
            "1.0.0",
        )
        .unwrap();
        for (tag, draft) in [
            ("different", false),
            ("expected", true),
            ("different", true),
        ] {
            Release::parse(
                json!({"id":1,"tag_name":tag,"draft":draft,"prerelease":false}),
                "expected",
                "1.0.0",
            )
            .unwrap_err();
        }
    }

    #[test]
    fn release_classification_matches_the_requested_version() {
        for version in ["1.0.0", "1.0.0-beta.1"] {
            for prerelease in [false, true] {
                let result = Release::parse(
                    json!({"id":1,"tag_name":"tag","draft":false,"prerelease":prerelease}),
                    "tag",
                    version,
                );
                assert_eq!(result.is_ok(), prerelease == version.contains('-'));
            }
        }
    }

    #[test]
    fn blank_tokens_do_not_suppress_fallback() {
        for blank in ["", " \t"] {
            assert_eq!(
                select_token(Some(blank.into()), Some("fallback".into())).as_deref(),
                Some("fallback")
            );
            assert!(select_token(Some(blank.into()), Some(blank.into())).is_none());
        }
        assert_eq!(
            select_token(Some("primary".into()), Some("fallback".into())).as_deref(),
            Some("primary")
        );
    }

    #[test]
    fn issue_markers_do_not_establish_ownership() {
        let title = "Release failed";
        let marker = "<!-- run:1:";
        for creator in ["owner", "other"] {
            let issue = json!({"number":7,"title":title,"body":marker,"user":{"login":creator}});
            assert_eq!(
                report_issue(&issue, "owner", title, marker).unwrap(),
                (creator == "owner").then_some(7)
            );
        }
        let issue = json!({"number":7,"title":title,"body":marker,"user":{"login":"owner"},"pull_request":{}});
        assert!(
            report_issue(&issue, "owner", title, marker)
                .unwrap()
                .is_none()
        );
    }
}
