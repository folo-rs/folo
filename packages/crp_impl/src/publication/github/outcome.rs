use serde::{Deserialize, Serialize};

use crate::publication::context::WorkflowRun;
use crate::publication::github::BatchArtifact;

/// One reconciliation attempt, including independently usable platform-batch artifacts.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GithubOutcome {
    pub schema_version: u32,
    pub publication_id: String,
    pub phase: String,
    pub dry_run: bool,
    pub complete: bool,
    pub packages: Vec<GithubPackage>,
    pub batches: Vec<BatchArtifact>,
    pub planned_targets: Vec<String>,
    pub errors: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<WorkflowRun>,
}

impl GithubOutcome {
    pub(crate) fn passed(&self, expected: usize) -> bool {
        self.errors.is_empty()
            && self.packages.len() == expected
            && self.packages.iter().all(|package| {
                package.state == GithubState::Complete
                    || (self.dry_run
                        && matches!(
                            package.state,
                            GithubState::WouldCreateTag | GithubState::WouldCreateRelease
                        ))
            })
    }
}

/// Per-package tag/release disposition; failures retain exact operator recovery identity.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GithubPackage {
    pub name: String,
    pub version: String,
    pub tag: String,
    pub state: GithubState,
    pub source: Option<String>,
    pub recovery_source: Option<String>,
    pub observed_version: Option<String>,
}

/// Separates completed reconciliation, read-only intent and failed/unattempted work.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GithubState {
    Pending,
    Complete,
    WouldCreateTag,
    WouldCreateRelease,
    Failed,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn record(state: GithubState) -> GithubPackage {
        GithubPackage {
            name: "tool".to_owned(),
            version: "1.0.0".to_owned(),
            tag: "tool-v1.0.0".to_owned(),
            state,
            source: None,
            recovery_source: None,
            observed_version: None,
        }
    }

    #[test]
    fn outcome_success_requires_complete_accounting_and_the_correct_execution_mode() {
        for dry_run in [false, true] {
            for state in [
                GithubState::Pending,
                GithubState::Complete,
                GithubState::WouldCreateTag,
                GithubState::WouldCreateRelease,
                GithubState::Failed,
            ] {
                let mut outcome = GithubOutcome {
                    schema_version: 1,
                    publication_id: "intent".to_owned(),
                    phase: "github".to_owned(),
                    dry_run,
                    complete: false,
                    packages: vec![record(state)],
                    batches: Vec::new(),
                    planned_targets: Vec::new(),
                    errors: Vec::new(),
                    github: None,
                };
                let expected = state == GithubState::Complete
                    || dry_run
                        && matches!(
                            state,
                            GithubState::WouldCreateTag | GithubState::WouldCreateRelease
                        );
                assert_eq!(outcome.passed(1), expected);
                assert!(!outcome.passed(2));
                outcome.errors.push("failure".to_owned());
                assert!(!outcome.passed(1));
            }
        }
    }
}
