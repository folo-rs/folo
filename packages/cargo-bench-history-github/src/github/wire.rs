use std::num::NonZero;

use ohno::AppError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::errors::InvalidResponseError;
use crate::github::{Comment, Comparison, Issue};

/// The directly read issue representation, including its current state.
#[derive(Deserialize)]
pub(crate) struct IssueResponse {
    pub(crate) number: NonZero<u64>,
    pub(crate) title: String,
    pub(crate) body: Option<String>,
    pub(crate) pull_request: Option<Value>,
    state: IssueState,
}

impl From<IssueResponse> for Issue {
    /// Transfers decoded issue state into the lifecycle model after response identity checks.
    fn from(value: IssueResponse) -> Self {
        Self {
            number: value.number.get(),
            title: value.title,
            body: value.body.unwrap_or_default(),
            open: matches!(value.state, IssueState::Open),
        }
    }
}

/// Unknown issue states must not be interpreted as open or absent.
#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum IssueState {
    Open,
    Closed,
}

/// An issue comment returned by GitHub's pull-request conversation endpoints.
#[derive(Deserialize)]
pub(crate) struct CommentResponse {
    pub(crate) id: NonZero<u64>,
    pub(crate) body: Option<String>,
}

impl From<CommentResponse> for Comment {
    /// Gives marker discovery the current body and stable comment update identity.
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

/// The complete writable content of a new issue.
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

/// Title and body describe the same captured publication date.
#[derive(Serialize)]
pub(crate) struct IssueUpdate<'a> {
    pub(crate) title: &'a str,
    pub(crate) body: &'a str,
}

/// Converts consistent GitHub relationship facts into usable forward-distance evidence.
///
/// Replacement guards and staleness messages must not infer freshness from reverse/divergent
/// comparisons or contradictory status/count combinations.
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
