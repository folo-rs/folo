//! Committed publication policy, independent of a particular workflow invocation.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crp_workspace::command::run_capture;
use ohno::AppError;
use serde::{Deserialize, Serialize};

use crate::{ParseTomlError, ReadFileError};

/// Effective policy captured in a publication manifest, without runner or source identities.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Configuration {
    schema_version: u32,
    repository: String,
    release_branch: String,
    targets: Vec<NativeTarget>,
}

impl Configuration {
    /// Reads the configured file relative to the selected Cargo workspace.
    pub fn load(
        workspace: &Path,
        configured_path: Option<&Path>,
    ) -> Result<(PathBuf, Self), AppError> {
        let path = configured_path.map_or_else(
            || workspace.join(".cargo").join("release_plan.toml"),
            |path| workspace.join(path),
        );
        let source =
            fs::read_to_string(&path).map_err(|error| ReadFileError::caused_by(&path, error))?;
        let config = Self::parse(&source, &path)?;
        // Git owns branch-name syntax; configuration parsing does not duplicate that grammar.
        let branch_check = run_capture(
            "git",
            &[
                "check-ref-format",
                &format!("refs/heads/{}", config.release_branch),
            ],
            workspace,
        );
        if let Err(error) = branch_check {
            if error.is_nonzero_exit() {
                return Err(ConfigurationError::caused_by(
                    &path,
                    format!("invalid release-branch {:?}", config.release_branch),
                    error,
                )
                .into());
            }
            return Err(error.into());
        }
        Ok((path, config))
    }

    fn parse(source: &str, path: &Path) -> Result<Self, AppError> {
        let config: Self = toml_edit::de::from_str(source)
            .map_err(|error| ParseTomlError::caused_by(path, error))?;
        config.validate(path)?;
        Ok(config)
    }

    pub(crate) fn validate(&self, path: &Path) -> Result<(), AppError> {
        if self.schema_version != CONFIG_SCHEMA_VERSION {
            return Err(ConfigurationError::new(
                path,
                format!(
                    "unsupported publication configuration schema {}",
                    self.schema_version
                ),
            )
            .into());
        }
        let parts: Vec<_> = self.repository.split('/').collect();
        if parts.len() != 2
            || parts.iter().any(|part| {
                part.is_empty()
                    || matches!(*part, "." | "..")
                    || !part.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
                    })
            })
        {
            return Err(ConfigurationError::new(
                path,
                "repository must be a GitHub owner/name".to_owned(),
            )
            .into());
        }
        if self.release_branch.is_empty() || self.release_branch.trim() != self.release_branch {
            return Err(ConfigurationError::new(
                path,
                "release-branch must name a branch without surrounding whitespace".to_owned(),
            )
            .into());
        }
        if self.targets.iter().collect::<BTreeSet<_>>().len() != self.targets.len() {
            return Err(ConfigurationError::new(
                path,
                "publication targets must not repeat".to_owned(),
            )
            .into());
        }
        Ok(())
    }

    #[must_use]
    pub fn repository(&self) -> &str {
        &self.repository
    }

    #[must_use]
    pub fn release_branch(&self) -> &str {
        &self.release_branch
    }

    /// Narrows workspace targets by a package restriction without adding new target support.
    pub fn binary_targets(
        &self,
        package: &str,
        restriction: Option<&[NativeTarget]>,
    ) -> Result<Vec<NativeTarget>, AppError> {
        let targets: Vec<_> = self
            .targets
            .iter()
            .copied()
            .filter(|target| restriction.is_none_or(|restriction| restriction.contains(target)))
            .collect();
        if targets.is_empty() {
            return Err(NoBinaryTarget::new(package.to_owned()).into());
        }
        Ok(targets)
    }
}

/// Native archive targets supported by the application; workflow releases choose their runners.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub enum NativeTarget {
    #[serde(rename = "x86_64-unknown-linux-gnu")]
    LinuxX64,
    #[serde(rename = "aarch64-unknown-linux-gnu")]
    LinuxArm64,
    #[serde(rename = "x86_64-pc-windows-msvc")]
    WindowsX64,
    #[serde(rename = "aarch64-pc-windows-msvc")]
    WindowsArm64,
    #[serde(rename = "aarch64-apple-darwin")]
    MacArm64,
}

impl NativeTarget {
    #[must_use]
    pub fn triple(self) -> &'static str {
        match self {
            Self::LinuxX64 => "x86_64-unknown-linux-gnu",
            Self::LinuxArm64 => "aarch64-unknown-linux-gnu",
            Self::WindowsX64 => "x86_64-pc-windows-msvc",
            Self::WindowsArm64 => "aarch64-pc-windows-msvc",
            Self::MacArm64 => "aarch64-apple-darwin",
        }
    }
}

/// Configuration format understood by publication preparation and the reusable action.
const CONFIG_SCHEMA_VERSION: u32 = 1;

#[ohno::error]
#[display("invalid publication configuration {}: {reason}", path.display())]
struct ConfigurationError {
    path: PathBuf,
    reason: String,
}

#[ohno::error]
#[display("binary package {package} has no selected publication target")]
struct NoBinaryTarget {
    package: String,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    const CONFIG: &str = "schema-version = 1\nrepository = 'example/widgets'\n\
                          release-branch = 'stable/releases'\n\
                          targets = ['x86_64-unknown-linux-gnu', 'x86_64-pc-windows-msvc']\n";

    #[test]
    fn resolves_repository_branch_and_package_target_intersection() {
        let config = Configuration::parse(CONFIG, Path::new("release.toml")).unwrap();
        assert_eq!(config.repository(), "example/widgets");
        assert_eq!(config.release_branch(), "stable/releases");
        assert_eq!(
            config.binary_targets("tool", None).unwrap(),
            [NativeTarget::LinuxX64, NativeTarget::WindowsX64]
        );
        assert_eq!(
            config
                .binary_targets(
                    "tool",
                    Some(&[NativeTarget::WindowsX64, NativeTarget::MacArm64])
                )
                .unwrap(),
            [NativeTarget::WindowsX64]
        );
        config
            .binary_targets("tool", Some(&[NativeTarget::MacArm64]))
            .unwrap_err();
        config.binary_targets("tool", Some(&[])).unwrap_err();
    }

    #[test]
    fn rejects_unknown_or_ambiguous_configuration() {
        for source in [
            CONFIG.replace("schema-version = 1", "schema-version = 2"),
            CONFIG.replace("example/widgets", "https://github.com/example/widgets"),
            CONFIG.replace("example/widgets", "../widgets"),
            CONFIG.replace("example/widgets", "example/"),
            CONFIG.replace("stable/releases", " stable "),
            CONFIG.replace("stable/releases", ""),
            CONFIG.replace("x86_64-pc-windows-msvc", "x86_64-unknown-linux-gnu"),
            CONFIG.replace("x86_64-pc-windows-msvc", "unsupported-target"),
            format!("{CONFIG}\nunknown = true"),
        ] {
            Configuration::parse(&source, Path::new("release.toml")).unwrap_err();
        }
    }

    #[test]
    fn library_only_workspaces_can_omit_binary_builds() {
        let config = Configuration::parse(
            "schema-version = 1\nrepository = 'example/libs'\nrelease-branch = 'main'\ntargets = []",
            Path::new("release.toml"),
        )
        .unwrap();
        config
            .binary_targets("unexpected-binary", None)
            .unwrap_err();
    }

    #[test]
    fn target_wire_identity_matches_cargo_triples() {
        for target in [
            NativeTarget::LinuxX64,
            NativeTarget::LinuxArm64,
            NativeTarget::WindowsX64,
            NativeTarget::WindowsArm64,
            NativeTarget::MacArm64,
        ] {
            let serialized = serde_json::to_string(&target).unwrap();
            assert_eq!(
                serde_json::from_str::<String>(&serialized).unwrap(),
                target.triple()
            );
            assert_eq!(
                serde_json::from_str::<NativeTarget>(&serialized).unwrap(),
                target
            );
        }
    }
}
