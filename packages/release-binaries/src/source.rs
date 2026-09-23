use std::path::PathBuf;

use ohno::AppError;
use serde::Deserialize;

use crate::model::{Binary, InvalidPlan};

/// Only Cargo fields needed to check the frozen package identity are decoded.
#[derive(Debug, Deserialize)]
pub(crate) struct Metadata {
    pub(crate) packages: Vec<Package>,
    pub(crate) workspace_members: Vec<String>,
    pub(crate) target_directory: PathBuf,
}

/// The source's Cargo package, including its unambiguous artifact identifier.
#[derive(Debug, Deserialize)]
pub(crate) struct Package {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) version: String,
    pub(crate) targets: Vec<CargoTarget>,
}

/// A Cargo target's name alone is insufficient: only binary targets are releasable.
#[derive(Debug, Deserialize)]
pub(crate) struct CargoTarget {
    pub(crate) name: String,
    pub(crate) kind: Vec<String>,
}

/// Cargo reports the actual executable rather than requiring target-path guesses.
#[derive(Debug, Deserialize)]
struct Artifact {
    reason: String,
    package_id: Option<String>,
    target: Option<CargoTarget>,
    executable: Option<PathBuf>,
}

impl Metadata {
    pub(crate) fn package(&self, binary: &Binary) -> Result<&Package, AppError> {
        let packages = self
            .packages
            .iter()
            .filter(|package| {
                package.name == binary.name
                    && package.version == binary.version
                    && self.workspace_members.contains(&package.id)
                    && package.targets.iter().any(|target| {
                        target.name == binary.bin && target.kind.iter().any(|kind| kind == "bin")
                    })
            })
            .collect::<Vec<_>>();
        if let [package] = packages.as_slice() {
            return Ok(package);
        }
        Err(InvalidPlan::new(format!(
            "{} does not identify exactly one workspace package/bin in its tagged source",
            binary.tag
        ))
        .into())
    }
}

pub(crate) fn executable(
    messages: &str,
    package_id: &str,
    binary: &str,
) -> Result<PathBuf, AppError> {
    let mut executable = None;
    for line in messages.lines().filter(|line| !line.trim().is_empty()) {
        let message: Artifact = serde_json::from_str(line)?;
        if message.reason == "compiler-artifact"
            && message.package_id.as_deref() == Some(package_id)
            && message.target.as_ref().is_some_and(|target| {
                target.name == binary && target.kind.iter().any(|kind| kind == "bin")
            })
            && let Some(path) = message.executable
            && executable.replace(path).is_some()
        {
            return Err(
                InvalidPlan::new("Cargo returned duplicate binary artifacts".to_owned()).into(),
            );
        }
    }
    executable.ok_or_else(|| {
        InvalidPlan::new(format!(
            "Successful Cargo build did not report executable for {package_id} / {binary}"
        ))
        .into()
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::model::tests::binary;

    #[test]
    fn metadata_requires_workspace_version_and_binary_target() {
        let metadata: Metadata = serde_json::from_value(serde_json::json!({
            "workspace_members": ["pkg"], "target_directory": "target",
            "packages": [{"id": "pkg", "name": "tool", "version": "1.2.3",
                "targets": [{"name": "tool-bin", "kind": ["bin"]}]}]
        }))
        .unwrap();
        assert_eq!(metadata.package(&binary("tool")).unwrap().id, "pkg");
        let mut wrong = binary("tool");
        wrong.version = "1.2.4".into();
        metadata.package(&wrong).unwrap_err();
        wrong = binary("tool");
        wrong.bin = "wrong".into();
        metadata.package(&wrong).unwrap_err();
    }

    #[test]
    fn artifact_identity_is_required_but_successfully_reused_artifacts_are_valid() {
        let message = serde_json::json!({
            "reason": "compiler-artifact", "package_id": "pkg",
            "target": {"name": "tool-bin", "kind": ["bin"]},
            "executable": "target/tool-bin", "fresh": true
        })
        .to_string();
        assert_eq!(
            executable(&message, "pkg", "tool-bin").unwrap(),
            PathBuf::from("target/tool-bin")
        );
        executable(&message, "other", "tool-bin").unwrap_err();
        executable(&message, "pkg", "other").unwrap_err();
        executable("", "pkg", "tool-bin").unwrap_err();
        executable("not json", "pkg", "tool-bin").unwrap_err();
        executable(&format!("{message}\n{message}"), "pkg", "tool-bin").unwrap_err();
    }
}
