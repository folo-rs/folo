use std::collections::BTreeMap;
use std::num::NonZero;
use std::path::{Path, PathBuf};

use cbh_config::rebase;
use ohno::AppError;
use serde::Deserialize;

use crate::action::errors::InvalidInput;
use crate::action::port::Host;
use crate::model::Repository;

/// Noncredential Actions context, with PR-head identity kept distinct from synthetic merge SHAs.
#[derive(Debug)]
pub(crate) struct Environment {
    values: BTreeMap<&'static str, String>,
    event: Event,
}

impl Environment {
    /// Captures execution identity needed for fork policy and publication fallbacks.
    ///
    /// The host supplies environment and event I/O; this snapshot never reads GitHub credentials.
    pub(crate) fn read(host: &impl Host, invocation_dir: &Path) -> Result<Self, AppError> {
        let mut values = BTreeMap::new();
        // This allow-list intentionally excludes both GitHub credential variables.
        for name in [
            "GITHUB_EVENT_NAME",
            "GITHUB_EVENT_PATH",
            "GITHUB_REPOSITORY",
            "GITHUB_RUN_ID",
            "GITHUB_RUN_ATTEMPT",
            "GITHUB_SERVER_URL",
            "GITHUB_SHA",
        ] {
            if let Some(value) = host.environment(name)?.filter(|value| !value.is_empty()) {
                values.insert(name, value);
            }
        }
        let event = if let Some(path) = values.get("GITHUB_EVENT_PATH") {
            serde_json::from_slice(&host.read(&rebase(invocation_dir, PathBuf::from(path)))?)
                .map_err(|error| {
                    InvalidInput::caused_by("GITHUB_EVENT_PATH", "invalid event", error)
                })?
        } else {
            Event::default()
        };
        if matches!(
            values.get("GITHUB_EVENT_NAME").map(String::as_str),
            Some("pull_request" | "pull_request_target")
        ) && event.pull_request.is_none()
        {
            return Err(InvalidInput::new(
                "GITHUB_EVENT_PATH",
                "PR events require their pull_request identity",
            )
            .into());
        }
        Ok(Self { values, event })
    }

    /// Applies the product's same-repository gate from event repository identities.
    ///
    /// This does not infer authorization from an OIDC subject or a token's permissions.
    pub(crate) fn fork(&self) -> bool {
        self.event.pull_request.as_ref().is_some_and(|pr| {
            match (&pr.head.repo, &pr.base.repo) {
                (Some(head), Some(base)) => !head.full_name.eq_ignore_ascii_case(&base.full_name),
                // A deleted or unavailable source repository cannot establish same-repository execution.
                _ => true,
            }
        })
    }

    /// Resolves the publication repository from explicit Actions context or the event envelope.
    pub(crate) fn repository(&self) -> Result<Repository, AppError> {
        self.value("GITHUB_REPOSITORY")
            .or_else(|| {
                self.event
                    .repository
                    .as_ref()
                    .map(|repo| repo.full_name.as_str())
            })
            .ok_or_else(|| {
                InvalidInput::new(
                    "GITHUB_REPOSITORY",
                    "repository context is required for publication",
                )
            })?
            .parse()
    }

    /// Returns captured context without performing another ambient environment read.
    pub(crate) fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    /// Supplies a real PR head before considering the non-PR `GITHUB_SHA` fallback.
    pub(crate) fn head(&self) -> Option<&str> {
        match &self.event.pull_request {
            Some(pr) => Some(pr.head.sha.as_str()),
            None => self.value("GITHUB_SHA"),
        }
    }

    /// Obtains the event's PR identity for comment commands whose input omits it.
    pub(crate) fn pull_request(&self) -> Option<NonZero<u64>> {
        self.event
            .pull_request
            .as_ref()?
            .number
            .or(self.event.number)
    }

    /// Freezes the PR's own target commit without assuming a default branch name.
    pub(crate) fn base(&self) -> Option<&str> {
        self.event
            .pull_request
            .as_ref()
            .map(|pr| pr.base.sha.as_str())
    }
}

/// Only execution identity is decoded; unrelated GitHub event fields remain GitHub-owned.
#[derive(Debug, Default, Deserialize)]
struct Event {
    repository: Option<EventRepository>,
    pull_request: Option<PullRequest>,
    number: Option<NonZero<u64>>,
}

/// The event's repository identity is used for same-repository policy and publication fallback.
#[derive(Debug, Deserialize)]
struct EventRepository {
    full_name: String,
}

/// PR events provide the real head and source repository, including on `pull_request_target`.
#[derive(Debug, Deserialize)]
struct PullRequest {
    number: Option<NonZero<u64>>,
    head: PullRef,
    base: PullRef,
}

/// A missing repository denotes unavailable provenance, not a same-repository approval.
#[derive(Debug, Deserialize)]
struct PullRef {
    sha: String,
    repo: Option<EventRepository>,
}
