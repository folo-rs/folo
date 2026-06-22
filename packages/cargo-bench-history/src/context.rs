//! The run context: the metadata that situates a [`ResultSet`](crate::ResultSet)
//! in time, in history (git), and in its execution environment (CI, toolchain,
//! host).
//!
//! Only the *commit* timestamp orders a series; the observation timestamp is
//! recorded for provenance and is never used for ordering.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// Metadata attached to every stored run.
#[non_exhaustive]
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RunContext {
    /// The timestamps describing this run (see [`Timestamps`]).
    pub timestamps: Timestamps,
    /// Information about the git commit the benchmarks were run against.
    pub git: GitInfo,
    /// Information about the continuous-integration environment, if any.
    pub ci: CiInfo,
    /// Toolchain and host/target identification.
    pub toolchain: ToolchainInfo,
    /// Version of the cargo-bench-history tool that produced this run.
    pub tool_version: String,
}

impl RunContext {
    /// Creates a run context from its components.
    #[must_use]
    pub fn new(
        timestamps: Timestamps,
        git: GitInfo,
        ci: CiInfo,
        toolchain: ToolchainInfo,
        tool_version: String,
    ) -> Self {
        Self {
            timestamps,
            git,
            ci,
            toolchain,
            tool_version,
        }
    }
}

/// The timestamps every run carries.
///
/// `commit` is the run's position on the timeline; `observation` is provenance
/// metadata and is never used for ordering. `commit` is the committer date of
/// the benchmarked commit (for a dirty snapshot, the commit it is based on).
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Timestamps {
    /// Committer date of the benchmarked commit; the run's timeline position. For
    /// a dirty snapshot it is the committer date of the commit it is based on.
    pub commit: Timestamp,
    /// Wall-clock time at which the run was observed (benchmarks executed and
    /// stored). Provenance only; never used to order a series.
    pub observation: Timestamp,
}

impl Timestamps {
    /// Creates a timestamp pair.
    #[must_use]
    pub fn new(commit: Timestamp, observation: Timestamp) -> Self {
        Self {
            commit,
            observation,
        }
    }
}

/// Identification of the git commit a run was measured against.
#[non_exhaustive]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct GitInfo {
    /// Full commit hash, if known.
    pub commit: Option<String>,
    /// Abbreviated commit hash, if known.
    pub short_commit: Option<String>,
    /// Branch name, if known.
    pub branch: Option<String>,
    /// Whether the working tree had uncommitted changes.
    pub dirty: bool,
}

/// Information about the continuous-integration environment a run executed in.
#[non_exhaustive]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct CiInfo {
    /// Which CI provider was detected.
    pub provider: CiProvider,
    /// Provider-specific run identifier, if any.
    pub run_id: Option<String>,
    /// Pull-request identifier, if any.
    pub pull_request: Option<String>,
}

/// The continuous-integration provider a run executed under.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CiProvider {
    /// Not running in recognized CI (developer machine).
    #[default]
    Local,
    /// GitHub Actions.
    GitHubActions,
    /// Azure DevOps pipelines.
    AzureDevOps,
}

/// Toolchain and platform identification for a run.
#[non_exhaustive]
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct ToolchainInfo {
    /// Resolved target triple where the benchmark binary actually ran.
    pub target_triple: String,
    /// Host triple of the tool process (differs from `target_triple` under WSL).
    pub host_triple: String,
    /// `rustc` version string, if detected.
    pub rustc_version: Option<String>,
}

/// Detects the CI provider and run metadata from environment-variable lookups.
///
/// `get` resolves an environment-variable name to its value, allowing the
/// detection to be unit-tested without touching the real process environment.
#[must_use]
pub fn detect_ci(get: impl Fn(&str) -> Option<String>) -> CiInfo {
    if get("GITHUB_ACTIONS").as_deref() == Some("true") {
        return CiInfo {
            provider: CiProvider::GitHubActions,
            run_id: get("GITHUB_RUN_ID"),
            pull_request: None,
        };
    }
    if get("TF_BUILD").is_some() {
        return CiInfo {
            provider: CiProvider::AzureDevOps,
            run_id: get("BUILD_BUILDID"),
            pull_request: get("SYSTEM_PULLREQUEST_PULLREQUESTID"),
        };
    }
    CiInfo::default()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name: &str| map.get(name).cloned()
    }

    #[test]
    fn detect_ci_recognizes_github_actions() {
        let env = env_from(&[("GITHUB_ACTIONS", "true"), ("GITHUB_RUN_ID", "42")]);
        let ci = detect_ci(env);
        assert_eq!(ci.provider, CiProvider::GitHubActions);
        assert_eq!(ci.run_id.as_deref(), Some("42"));
    }

    #[test]
    fn detect_ci_recognizes_azure_devops() {
        let env = env_from(&[
            ("TF_BUILD", "True"),
            ("BUILD_BUILDID", "7"),
            ("SYSTEM_PULLREQUEST_PULLREQUESTID", "99"),
        ]);
        let ci = detect_ci(env);
        assert_eq!(ci.provider, CiProvider::AzureDevOps);
        assert_eq!(ci.run_id.as_deref(), Some("7"));
        assert_eq!(ci.pull_request.as_deref(), Some("99"));
    }

    #[test]
    fn detect_ci_defaults_to_local() {
        let ci = detect_ci(env_from(&[]));
        assert_eq!(ci.provider, CiProvider::Local);
    }
}
