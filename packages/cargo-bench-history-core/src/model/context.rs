//! The run context: the metadata that situates a [`Run`](crate::model::Run) in
//! history (git) and in its execution environment (automation provider,
//! toolchain, host).
//!
//! A run is positioned on its series timeline purely by git topology — the
//! first-parent index of its commit, resolved live during analysis from the
//! stored commit ID. The object records no commit timestamp of its own; the
//! observation timestamp it does carry is provenance and is never used for
//! ordering or windowing.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// Metadata attached to every stored run.
///
/// A run's timeline position comes from git topology (keyed by the commit ID in
/// [`git`](Self::git)), not from any stored timestamp. `observation` is provenance
/// metadata and is never used for ordering or windowing.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunContext {
    /// Wall-clock time at which the run was observed (benchmarks executed and
    /// stored). Provenance only; never used to order or window a series.
    pub observation: Timestamp,
    /// Information about the git commit the benchmarks were run against.
    pub git: GitInfo,
    /// Information about the execution environment (developer machine or
    /// automation provider).
    pub env: EnvironmentInfo,
    /// Toolchain and target identification.
    pub toolchain: ToolchainInfo,
    /// Version of the cargo-bench-history tool that produced this run.
    pub tool_version: String,
}

impl RunContext {
    /// Creates a run context from its components.
    #[must_use]
    pub fn new(
        observation: Timestamp,
        git: GitInfo,
        env: EnvironmentInfo,
        toolchain: ToolchainInfo,
        tool_version: String,
    ) -> Self {
        Self {
            observation,
            git,
            env,
            toolchain,
            tool_version,
        }
    }
}

/// Identification of the git commit a run was measured against.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitInfo {
    /// Full commit hash, if known.
    pub commit: Option<String>,
    /// Branch name, if known.
    pub branch: Option<String>,
    /// Whether the working tree had uncommitted changes.
    pub dirty: bool,
}

/// Information about the environment a run executed in.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct EnvironmentInfo {
    /// Which environment was detected.
    pub provider: EnvironmentProvider,
    /// Provider-specific run identifier, if any.
    pub run_id: Option<String>,
    /// Pull-request identifier, if any.
    pub pull_request: Option<String>,
}

/// The environment a run executed under.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentProvider {
    /// A developer machine, not a recognized automation provider.
    #[default]
    Local,
    /// GitHub Actions.
    #[serde(rename = "github_actions")]
    GitHubActions,
    /// Azure DevOps pipelines.
    #[serde(rename = "azure_devops")]
    AzureDevOps,
}

/// Toolchain and platform identification for a run.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolchainInfo {
    /// Target triple the benchmark binary ran on. The tool always runs on the
    /// same OS it benchmarks (under WSL, the Linux side), so this is the host
    /// triple `rustc` reports.
    pub target_triple: String,
    /// `rustc` version string, if detected.
    pub rustc_version: Option<String>,
}

/// Detects the execution environment and run metadata from environment-variable
/// lookups.
///
/// `get` resolves an environment-variable name to its value, allowing the
/// detection to be unit-tested without touching the real process environment.
#[must_use]
pub fn detect_environment(get: impl Fn(&str) -> Option<String>) -> EnvironmentInfo {
    if get("GITHUB_ACTIONS").as_deref() == Some("true") {
        return EnvironmentInfo {
            provider: EnvironmentProvider::GitHubActions,
            run_id: get("GITHUB_RUN_ID"),
            pull_request: None,
        };
    }
    if get("TF_BUILD").is_some() {
        return EnvironmentInfo {
            provider: EnvironmentProvider::AzureDevOps,
            run_id: get("BUILD_BUILDID"),
            pull_request: get("SYSTEM_PULLREQUEST_PULLREQUESTID"),
        };
    }
    EnvironmentInfo::default()
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
    fn detect_environment_recognizes_github_actions() {
        let env = env_from(&[("GITHUB_ACTIONS", "true"), ("GITHUB_RUN_ID", "42")]);
        let detected = detect_environment(env);
        assert_eq!(detected.provider, EnvironmentProvider::GitHubActions);
        assert_eq!(detected.run_id.as_deref(), Some("42"));
    }

    #[test]
    fn detect_environment_recognizes_azure_devops() {
        let env = env_from(&[
            ("TF_BUILD", "True"),
            ("BUILD_BUILDID", "7"),
            ("SYSTEM_PULLREQUEST_PULLREQUESTID", "99"),
        ]);
        let detected = detect_environment(env);
        assert_eq!(detected.provider, EnvironmentProvider::AzureDevOps);
        assert_eq!(detected.run_id.as_deref(), Some("7"));
        assert_eq!(detected.pull_request.as_deref(), Some("99"));
    }

    #[test]
    fn detect_environment_defaults_to_local() {
        let detected = detect_environment(env_from(&[]));
        assert_eq!(detected.provider, EnvironmentProvider::Local);
    }

    #[test]
    fn provider_serializes_brand_names_without_mangling() {
        // The brand names must serialize as recognizable tokens, not the
        // `snake_case` split (`git_hub_actions` / `azure_dev_ops`) the default
        // rename would produce.
        let cases = [
            (EnvironmentProvider::Local, "\"local\""),
            (EnvironmentProvider::GitHubActions, "\"github_actions\""),
            (EnvironmentProvider::AzureDevOps, "\"azure_devops\""),
        ];
        for (provider, wire) in cases {
            let serialized = serde_json::to_string(&provider).unwrap();
            assert_eq!(serialized, wire);
            let parsed: EnvironmentProvider = serde_json::from_str(wire).unwrap();
            assert_eq!(parsed, provider);
        }
    }
}
