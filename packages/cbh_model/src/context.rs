//! The run context: the metadata that situates a [`Run`](crate::Run) in
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

use super::identifiers::TargetTriple;

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
    /// Host hardware provenance: the machine fingerprint and the factors it
    /// derives from. Write-only — nothing reads it back today; it is recorded so
    /// that a later change in a machine key can be traced to the specific factor
    /// that changed. `None` on runs written before this field existed (and on the
    /// many non-collect construction sites that do not probe hardware).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<MachineInfo>,
}

impl RunContext {
    /// Creates a run context from its components.
    ///
    /// The host [`machine`](Self::machine) provenance is left absent; `collect`
    /// sets it after construction (it is the only site that probes hardware), so
    /// the many other callers need not thread a value they cannot supply.
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
            machine: None,
        }
    }
}

/// Host hardware provenance recorded with a run: the machine fingerprint and the
/// factors it hashes.
///
/// Write-only metadata. Nothing reads it back today; it exists so that a later
/// change in a machine key can be traced to the specific hardware factor that
/// changed (for example, a runner pool swapping CPU models). It records the host's
/// auto-detected fingerprint and is independent of any `--machine-key` override
/// used to partition storage.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MachineInfo {
    /// Number of logical processors the host reported.
    pub processors: usize,
    /// Number of NUMA memory regions the host reported.
    pub memory_regions: usize,
    /// Distinct processor model strings the host reported, sorted ascending.
    /// Defaults to empty when reading older records that predate this factor.
    #[serde(default)]
    pub processor_models: Vec<String>,
    /// Histogram of the per-processor relative speeds the host reported, as
    /// `(speed, count)` pairs sorted ascending by speed. Defaults to empty when
    /// reading older records that predate this factor.
    #[serde(default)]
    pub processor_speeds: Vec<(u64, usize)>,
    /// The hardware fingerprint these factors hash to: the host's auto-detected
    /// identity, recorded regardless of the machine key the run was partitioned
    /// under (see the type-level note).
    pub fingerprint: String,
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
    pub target_triple: TargetTriple,
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
    fn machine_info_is_omitted_when_absent_and_restored_when_present() {
        let epoch = "2024-01-01T00:00:00Z".parse().unwrap();
        let context = RunContext::new(
            epoch,
            GitInfo::default(),
            EnvironmentInfo::default(),
            ToolchainInfo::default(),
            "0.0.1".to_owned(),
        );

        // Absent host provenance must not appear on the wire, so old readers and
        // records stay byte-compatible.
        let json = serde_json::to_string(&context).unwrap();
        assert!(!json.contains("machine"), "{json}");
        assert_eq!(serde_json::from_str::<RunContext>(&json).unwrap(), context);

        // A populated value round-trips intact.
        let mut with_machine = context;
        with_machine.machine = Some(MachineInfo {
            processors: 8,
            memory_regions: 1,
            processor_models: vec!["Test CPU 3000".to_owned()],
            processor_speeds: vec![(3141, 8)],
            fingerprint: "test-fingerprint".to_owned(),
        });
        let json = serde_json::to_string(&with_machine).unwrap();
        assert!(
            json.contains("\"fingerprint\":\"test-fingerprint\""),
            "{json}"
        );
        assert_eq!(
            serde_json::from_str::<RunContext>(&json).unwrap(),
            with_machine
        );
    }

    #[test]
    fn run_context_without_machine_field_deserializes_to_none() {
        // A record written before the host-provenance field existed omits it
        // entirely; it must still parse, leaving `machine` absent.
        let json = r#"{
            "observation": "2024-01-01T00:00:00Z",
            "git": {"commit": null, "branch": null, "dirty": false},
            "env": {"provider": "local", "run_id": null, "pull_request": null},
            "toolchain": {"target_triple": "", "rustc_version": null},
            "tool_version": "0.0.1"
        }"#;
        let context: RunContext = serde_json::from_str(json).unwrap();
        assert_eq!(context.machine, None);
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
