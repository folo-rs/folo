use std::num::NonZero;

use ohno::AppError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errors::InvalidResponseError;
use crate::github::{Comment, Comparison, Issue};

/// The issue-list representation, which also includes pull requests.
#[derive(Deserialize)]
pub(crate) struct IssueResponse {
    pub(crate) number: NonZero<u64>,
    pub(crate) title: String,
    pub(crate) body: Option<String>,
    pub(crate) pull_request: Option<Value>,
    user: Option<IssueAuthor>,
}

impl From<IssueResponse> for Issue {
    fn from(value: IssueResponse) -> Self {
        Self {
            number: value.number.get(),
            title: value.title,
            body: value.body.unwrap_or_default(),
            bot_authored: value.user.is_some_and(|user| user.kind == "Bot"),
        }
    }
}

/// The API's author classification is the evidence needed for opt-in legacy title adoption.
#[derive(Deserialize)]
struct IssueAuthor {
    #[serde(rename = "type")]
    kind: String,
}

/// An issue comment returned by GitHub's pull-request conversation endpoints.
#[derive(Deserialize)]
pub(crate) struct CommentResponse {
    pub(crate) id: NonZero<u64>,
    pub(crate) body: Option<String>,
}

impl From<CommentResponse> for Comment {
    fn from(value: CommentResponse) -> Self {
        Self {
            id: value.id.get(),
            body: value.body.unwrap_or_default(),
        }
    }
}

/// Only the pull-request head is needed for report freshness.
#[derive(Deserialize)]
pub(crate) struct PullResponse {
    pub(crate) head: PullHead,
}

/// The head identity nested inside a pull-request response.
#[derive(Deserialize)]
pub(crate) struct PullHead {
    pub(crate) sha: String,
}

/// GitHub's directional relationship, not merely a count of unique head commits.
#[derive(Deserialize)]
pub(crate) struct CompareResponse {
    status: CompareStatus,
    ahead_by: u64,
    behind_by: u64,
}

/// Relationships that determine whether a linear distance can be reported.
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum CompareStatus {
    Ahead,
    Identical,
    Behind,
    Diverged,
}

/// The complete writable content of a new rolling issue.
#[derive(Serialize)]
pub(crate) struct IssueWrite<'a> {
    pub(crate) title: &'a str,
    pub(crate) body: &'a str,
}

/// The writable content of a pull-request conversation comment.
#[derive(Serialize)]
pub(crate) struct BodyWrite<'a> {
    pub(crate) body: &'a str,
}

/// A rolling issue update can preserve the existing title.
#[derive(Serialize)]
pub(crate) struct IssueUpdate<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) title: Option<&'a str>,
    pub(crate) body: &'a str,
}

/// Closes an issue without replacing its report content.
#[derive(Serialize)]
pub(crate) struct IssueStateWrite<'a> {
    pub(crate) state: &'a str,
}

pub(crate) fn comparison(value: &CompareResponse) -> Result<Comparison, AppError> {
    let ahead_by = match value.status {
        CompareStatus::Ahead if value.ahead_by > 0 && value.behind_by == 0 => Some(value.ahead_by),
        CompareStatus::Identical if value.ahead_by == 0 && value.behind_by == 0 => Some(0),
        // A reverse or divergent relationship cannot say how many commits the analyzed
        // revision is behind the head. In particular, zero must not imply freshness.
        CompareStatus::Behind if value.ahead_by == 0 && value.behind_by > 0 => None,
        CompareStatus::Diverged if value.ahead_by > 0 && value.behind_by > 0 => None,
        _ => return Err(InvalidResponseError::new("comparing commits").into()),
    };
    Ok(Comparison { ahead_by })
}
