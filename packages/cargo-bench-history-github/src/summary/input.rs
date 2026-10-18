use std::path::PathBuf;

use serde::Deserialize;

use crate::result::Execution;

/// Job facts supplied by orchestration, separate from analyzer-owned evidence.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SummaryInput {
    pub(crate) execution: Execution,
    pub(crate) publication: Publication,
    pub(crate) run_url: String,
    pub(crate) artifact_url: Option<String>,
    pub(crate) result: ResultInput,
}

/// The state of GitHub publication, not collection or storage.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Publication {
    Disabled,
    Completed,
    Failed,
    NotAttempted,
    NotRequested,
}

impl Publication {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Disabled => "Disabled; collection and storage are independent",
            Self::Completed => "Request completed; lifecycle policy may leave posts unchanged",
            Self::Failed => "Failed; inspect the publication job",
            Self::NotAttempted => "Not attempted",
            Self::NotRequested => "Not requested (collection only)",
        }
    }
}

/// Only an explicit report branch may carry an analysis verdict.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ResultInput {
    Analysis {
        report_file: PathBuf,
        analyzed_sha: String,
        expected_platforms: String,
        completed_platforms: String,
        #[serde(default)]
        packages: Vec<String>,
        observed_head: Option<String>,
    },
    Unavailable {
        reason: UnavailableReason,
    },
    Backfill {
        from: String,
        to: String,
        expected_platforms: String,
        platforms: Vec<BackfillPlatform>,
    },
}

/// Explicit no-analysis reasons are not inferred from missing files or exit codes.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum UnavailableReason {
    EmptyScope,
    ForkPullRequest,
    NoEligibleCommit,
    ReportsUnavailable,
    CollectionOnly,
}

/// One platform's terminal job state and optional, same-attempt completion artifact.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BackfillPlatform {
    pub(crate) platform: String,
    pub(crate) execution: Execution,
    pub(crate) completion_file: Option<PathBuf>,
}

/// Optional diagnostics already present in the core JSON, not a new analysis schema.
#[derive(Debug, Deserialize)]
pub(crate) struct ReportDetails {
    pub(crate) hint: Option<String>,
    #[serde(default)]
    pub(crate) sets: Vec<ReportSet>,
}

/// Actual selected partitions, rather than guessed host defaults.
#[derive(Debug, Deserialize)]
pub(crate) struct ReportSet {
    pub(crate) engine: String,
    pub(crate) target_triple: String,
    pub(crate) machine_key: String,
}
