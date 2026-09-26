//! Hosted attempt attribution for outcomes, separate from immutable publication intent.

use std::env;
use std::fmt::Write as _;
use std::num::NonZero;
use std::path::Path;

use crp_diag::Verbose;
use crp_workspace::git::GitRepo;
use ohno::AppError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::publication::config::Configuration;
use crate::publication::manifest::InvalidManifest;
use crate::publication::packages::PublicationWorkspace;
use crate::publication::prepare::fetch_release_line;

/// Resolved facts shared by local version planning and the reusable workflow's preparation job.
#[derive(Debug, Serialize)]
struct ReleaseContext {
    schema_version: u32,
    repository: String,
    release_branch: String,
    release_base: String,
    head: String,
    workspace_manifest: String,
    config_path: String,
    concurrency_group: String,
}

pub fn release_context(
    manifest: &Path,
    configured_path: Option<&Path>,
    base: Option<&str>,
    verbose: Verbose<'_>,
) -> Result<String, AppError> {
    let workspace = PublicationWorkspace::load(manifest)?;
    let (config_path, config) = Configuration::load(workspace.root(), configured_path)?;
    let git = GitRepo::discover(workspace.root())?;
    let root = dunce::canonicalize(git.root())?;
    let relative = |path: &Path| -> Result<String, AppError> {
        let path = dunce::canonicalize(path)?;
        let path = path.strip_prefix(&root).map_err(|error| {
            InvalidManifest::caused_by(
                "release context inputs must stay inside the repository".to_owned(),
                error,
            )
        })?;
        path.to_str()
            .map(|path| path.replace('\\', "/"))
            .ok_or_else(|| {
                InvalidManifest::new("release context paths must be UTF-8".to_owned()).into()
            })
    };
    let release_base = match base {
        Some(base) => git.rev_parse(&format!("{base}^{{commit}}"))?,
        None => fetch_release_line(git.root(), &config)?,
    };
    let workspace_manifest = relative(&workspace.root().join("Cargo.toml"))?;
    let group = concurrency_group(
        config.repository(),
        config.release_branch(),
        &workspace_manifest,
    )?;
    verbose.note(||format!(
        "Release context uses {} branch {} at {release_base}; workspace {workspace_manifest} selects concurrency group {group}.",
        config.repository(),config.release_branch()
    ));
    Ok(serde_json::to_string(&ReleaseContext {
        schema_version: 1,
        repository: config.repository().to_owned(),
        release_branch: config.release_branch().to_owned(),
        release_base,
        head: git.head()?,
        workspace_manifest,
        config_path: relative(&config_path)?,
        concurrency_group: group,
    })?)
}

fn concurrency_group(repository: &str, branch: &str, manifest: &str) -> Result<String, AppError> {
    // GitHub repository names are case-insensitive; Git branches and repository paths are not.
    let bytes = serde_json::to_vec(&(repository.to_ascii_lowercase(), branch, manifest))?;
    let mut hash = String::new();
    for byte in Sha256::digest(bytes) {
        write!(hash, "{byte:02x}")?;
    }
    Ok(format!("cargo-release-plan-{hash}"))
}

/// GitHub's run and attempt identity, used to select receipts without mutating earlier ones.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRun {
    pub run_id: NonZero<u64>,
    pub run_attempt: NonZero<u32>,
}

impl WorkflowRun {
    pub fn capture() -> Result<Option<Self>, AppError> {
        let id = optional_environment("GITHUB_RUN_ID")?;
        let attempt = optional_environment("GITHUB_RUN_ATTEMPT")?;
        let hosted = optional_environment("GITHUB_ACTIONS")?.as_deref() == Some("true");
        Self::parse(id.as_deref(), attempt.as_deref(), hosted)
    }

    fn parse(
        id: Option<&str>,
        attempt: Option<&str>,
        hosted: bool,
    ) -> Result<Option<Self>, AppError> {
        match (id, attempt) {
            (None, None) if !hosted => Ok(None),
            (Some(id), Some(attempt)) => Ok(Some(Self {
                run_id: id
                    .parse()
                    .map_err(|error| InvalidWorkflowContext::caused_by("GITHUB_RUN_ID", error))?,
                run_attempt: attempt.parse().map_err(|error| {
                    InvalidWorkflowContext::caused_by("GITHUB_RUN_ATTEMPT", error)
                })?,
            })),
            _ => Err(InvalidWorkflowContext::new("GITHUB_RUN_ID and GITHUB_RUN_ATTEMPT").into()),
        }
    }
}

fn optional_environment(name: &'static str) -> Result<Option<String>, AppError> {
    match env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(InvalidWorkflowContext::caused_by(name, error).into()),
    }
}

#[ohno::error]
#[display("invalid or incomplete GitHub workflow context: {field}")]
struct InvalidWorkflowContext {
    field: &'static str,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn concurrency_groups_scope_the_workspace_not_a_source_commit_or_attempt() {
        let group = concurrency_group("Example/Tools", "stable", "rust/Cargo.toml").unwrap();
        assert_eq!(
            group,
            concurrency_group("example/tools", "stable", "rust/Cargo.toml").unwrap()
        );
        assert_ne!(
            group,
            concurrency_group("example/tools", "Stable", "rust/Cargo.toml").unwrap()
        );
        assert_ne!(
            group,
            concurrency_group("example/tools", "stable", "other/Cargo.toml").unwrap()
        );
    }

    #[test]
    fn hosted_context_requires_both_positive_identifiers() {
        assert!(WorkflowRun::parse(None, None, false).unwrap().is_none());
        WorkflowRun::parse(None, None, true).unwrap_err();
        for (id, attempt) in [
            (Some("1"), None),
            (None, Some("1")),
            (Some("0"), Some("1")),
            (Some("1"), Some("0")),
            (Some("bad"), Some("1")),
        ] {
            WorkflowRun::parse(id, attempt, false).unwrap_err();
        }
        let context = WorkflowRun::parse(Some("123"), Some("2"), true)
            .unwrap()
            .unwrap();
        assert_eq!(context.run_id.get(), 123);
        assert_eq!(context.run_attempt.get(), 2);
    }
}
