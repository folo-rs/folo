//! Package-driven publication validation over Cargo's unresolved metadata.

use std::path::{Path, PathBuf};

use crp_diag::Verbose;
use crp_workspace::metadata::{
    capture_metadata, permits_publication_to, reject_legacy_groups, validate_release_plan_metadata,
};
use ohno::AppError;
use serde::Deserialize;
use serde_json::Value;

use crate::ParseMetadataError;
use crate::publication::config::{Configuration, NativeTarget};

/// Cargo workspace facts needed to validate publication before any remote write.
#[derive(Debug, Deserialize)]
pub struct PublicationWorkspace {
    workspace_root: PathBuf,
    workspace_members: Vec<String>,
    packages: Vec<PublicationPackage>,
    #[serde(default)]
    metadata: Value,
}

impl PublicationWorkspace {
    /// Checks a historical tag's package identity without imposing current publication policy.
    pub(crate) fn contains_release(&self, name: &str, version: &str, binary: Option<&str>) -> bool {
        self.packages
            .iter()
            .filter(|package| {
                package.name == name
                    && package.version == version
                    && self.workspace_members.contains(&package.id)
                    && !package.publish.as_ref().is_some_and(Vec::is_empty)
                    && binary.is_none_or(|binary| {
                        package.targets.iter().any(|target| {
                            target.name == binary && target.kind.iter().any(|kind| kind == "bin")
                        })
                    })
            })
            .count()
            == 1
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.workspace_root
    }

    pub fn load(manifest: &Path) -> Result<Self, AppError> {
        let source = capture_metadata(manifest)?;
        serde_json::from_slice(&source).map_err(|error| ParseMetadataError::caused_by(error).into())
    }

    /// Returns all publication requests, including versions already available remotely.
    pub fn requests(&self, config: &Configuration) -> Result<Vec<PackageRequest>, AppError> {
        config.validate(Path::new(".cargo/release_plan.toml"))?;
        let mut requests = Vec::new();
        for package in self.publication_packages(None)? {
            if !permits_publication_to(package.publish.as_deref(), "crates-io") {
                return Err(PackageConfigurationError::new(
                    &package.name,
                    "the package is not publishable to crates.io".to_owned(),
                )
                .into());
            }
            let targets: Vec<_> = package
                .targets
                .iter()
                .filter(|target| target.kind.iter().any(|kind| kind == "bin"))
                .collect();
            let binary = match targets.as_slice() {
                [] => None,
                [target] => {
                    validate_binary_metadata(package, config)?;
                    let restriction = package
                        .metadata
                        .get("release-plan")
                        .and_then(|metadata| metadata.get("release-targets"))
                        .map(|value| serde_json::from_value::<Vec<NativeTarget>>(value.clone()))
                        .transpose()
                        .map_err(|error| {
                            InvalidPackageTargets::caused_by(package.name.clone(), error)
                        })?;
                    Some(BinaryRequest {
                        name: target.name.clone(),
                        targets: config.binary_targets(&package.name, restriction.as_deref())?,
                    })
                }
                _ => {
                    return Err(PackageConfigurationError::new(
                        &package.name,
                        "exactly one installable binary is supported".to_owned(),
                    )
                    .into());
                }
            };
            requests.push(PackageRequest {
                name: package.name.clone(),
                version: package.version.clone(),
                manifest: package.manifest_path.clone(),
                binary,
            });
        }
        requests.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(requests)
    }

    /// Registry preflight uses destination eligibility, not binary-build validation.
    pub(crate) fn registry_targets(
        &self,
        selected: Option<&[String]>,
    ) -> Result<Vec<String>, AppError> {
        let mut targets: Vec<_> = self
            .publication_packages(selected)?
            .into_iter()
            .filter(|package| permits_publication_to(package.publish.as_deref(), "crates-io"))
            .map(|package| package.name.clone())
            .collect();
        targets.sort();
        Ok(targets)
    }

    fn publication_packages(
        &self,
        selected: Option<&[String]>,
    ) -> Result<Vec<&PublicationPackage>, AppError> {
        reject_legacy_groups(&self.metadata)?;
        let mut packages = Vec::new();
        for package in &self.packages {
            if !self.workspace_members.contains(&package.id)
                || selected.is_some_and(|names| !names.contains(&package.name))
            {
                continue;
            }
            reject_legacy_groups(&package.metadata)?;
            validate_release_plan_metadata(&package.name, &package.metadata)?;
            if package.publish.as_ref().is_some_and(Vec::is_empty) {
                continue;
            }
            packages.push(package);
        }
        Ok(packages)
    }
}

/// Validated package selection before repository-relative paths enter publication intent.
#[derive(Debug)]
pub struct PackageRequest {
    pub name: String,
    pub version: String,
    pub manifest: PathBuf,
    pub binary: Option<BinaryRequest>,
}

/// The executable and native builds promised for one binary package version.
#[derive(Debug)]
pub struct BinaryRequest {
    pub name: String,
    pub targets: Vec<NativeTarget>,
}

/// Cargo package facts needed for publication selection and archive naming.
#[derive(Debug, Deserialize)]
struct PublicationPackage {
    id: String,
    name: String,
    version: String,
    manifest_path: PathBuf,
    repository: Option<String>,
    publish: Option<Vec<String>>,
    metadata: Value,
    targets: Vec<PublicationTarget>,
}

/// The Cargo target identity, independent of whether its default-feature build succeeds.
#[derive(Debug, Deserialize)]
struct PublicationTarget {
    name: String,
    kind: Vec<String>,
}

fn validate_binary_metadata(
    package: &PublicationPackage,
    config: &Configuration,
) -> Result<(), AppError> {
    // Feature selection and buildability belong to the actual default-feature Cargo build.
    // This metadata check only confirms the cheap naming/layout inputs publication owns.
    let repository = format!("https://github.com/{}", config.repository());
    if package.repository.as_deref() != Some(repository.as_str()) {
        return Err(PackageConfigurationError::new(
            &package.name,
            format!("package.repository must be {repository} for cargo-binstall asset URLs"),
        )
        .into());
    }
    // Match the publisher's package/version tag, target-qualified ZIP name and root executable.
    // Ref: book/src/reference/configuration.md, "Binary metadata and naming".
    let required = [
        (
            "pkg-url",
            "{ repo }/releases/download/{ name }-v{ version }/{ name }-v{ version }-{ target }.zip",
        ),
        ("bin-dir", "{ bin }{ binary-ext }"),
        ("pkg-fmt", "zip"),
    ];
    for (field, expected) in required {
        let actual = package
            .metadata
            .get("binstall")
            .and_then(|metadata| metadata.get(field))
            .and_then(Value::as_str);
        if actual != Some(expected) {
            return Err(PackageConfigurationError::new(
                &package.name,
                format!("metadata.binstall.{field} must be {expected:?}"),
            )
            .into());
        }
    }
    Ok(())
}

/// Validates publication configuration without resolving dependencies or observing a registry.
pub fn check_publication(
    manifest: &Path,
    config: &Path,
    verbose: Verbose<'_>,
) -> Result<(), AppError> {
    let workspace = PublicationWorkspace::load(manifest)?;
    let (path, config) = Configuration::load(&workspace.workspace_root, Some(config))?;
    let requests = workspace.requests(&config)?;
    verbose.note(|| {
        format!(
            "Publication configuration {} selects repository {} and release branch {}; \
         validating every publishable workspace package, independently of registry availability.",
            path.display(),
            config.repository(),
            config.release_branch()
        )
    });
    for request in requests {
        if let Some(binary) = request.binary {
            verbose.note(|| {
                format!(
                    "{}@{} publishes executable {} with default features on [{}], \
                 after intersecting workspace targets with its package restriction.",
                    request.name,
                    request.version,
                    binary.name,
                    binary
                        .targets
                        .iter()
                        .map(|target| target.triple())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            });
        }
    }
    Ok(())
}

#[ohno::error]
#[display("invalid publication input for package {package}: {reason}")]
struct PackageConfigurationError {
    package: String,
    reason: String,
}

#[ohno::error]
#[display("invalid release-targets metadata for package {package}")]
struct InvalidPackageTargets {
    package: String,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::indexing_slicing,
    reason = "Tests mutate named fields in their own fixed JSON fixture."
)]
mod tests {
    use serde_json::json;

    use super::*;

    fn configuration() -> Configuration {
        serde_json::from_value(json!({
            "schema-version": 1,
            "repository": "example/tools",
            "release-branch": "main",
            "targets": ["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"]
        }))
        .unwrap()
    }

    fn package() -> Value {
        json!({
            "id": "tool-id", "name": "tool", "version": "1.0.0",
            "manifest_path": "tools/Cargo.toml",
            "repository": "https://github.com/example/tools",
            "publish": null,
            "targets": [{"name": "different-executable", "kind": ["bin"]}],
            "metadata": {
                "binstall": {
                    "pkg-url": "{ repo }/releases/download/{ name }-v{ version }/{ name }-v{ version }-{ target }.zip",
                    "bin-dir": "{ bin }{ binary-ext }", "pkg-fmt": "zip"
                }
            }
        })
    }

    fn requests(package: &Value) -> Result<Vec<PackageRequest>, AppError> {
        let workspace: PublicationWorkspace = serde_json::from_value(json!({
            "workspace_root": "workspace",
            "workspace_members": ["tool-id"],
            "packages": [package]
        }))
        .unwrap();
        workspace.requests(&configuration())
    }

    #[test]
    fn distinguishes_package_executable_and_effective_targets() {
        let mut package = package();
        package["metadata"]["release-plan"] =
            json!({"release-targets": ["x86_64-pc-windows-msvc"]});
        let requests = requests(&package).unwrap();
        let request = requests.first().unwrap();
        assert_eq!(request.name, "tool");
        assert_eq!(request.version, "1.0.0");
        assert_eq!(request.manifest, PathBuf::from("tools/Cargo.toml"));
        let binary = request.binary.as_ref().unwrap();
        assert_eq!(binary.name, "different-executable");
        assert_eq!(binary.targets, [NativeTarget::WindowsX64]);
    }

    #[test]
    fn rejects_ambiguous_binary_and_invalid_archive_promises() {
        let mut cases = Vec::new();
        for (field, value) in [
            ("repository", json!("https://github.com/another/repository")),
            ("publish", json!(["another-registry"])),
            (
                "targets",
                json!([{"name":"a","kind":["bin"]},{"name":"b","kind":["bin"]}]),
            ),
        ] {
            let mut package = package();
            package[field] = value;
            cases.push(package);
        }
        for field in ["pkg-url", "bin-dir", "pkg-fmt"] {
            let mut package = package();
            package["metadata"]["binstall"][field] = json!("different");
            cases.push(package);
        }
        for targets in [
            json!([]),
            json!(["aarch64-apple-darwin"]),
            json!(["unknown"]),
        ] {
            let mut package = package();
            package["metadata"]["release-plan"] = json!({"release-targets": targets});
            cases.push(package);
        }
        let mut malformed = package();
        malformed["metadata"]["release-plan"] = json!(true);
        cases.push(malformed);
        for package in cases {
            requests(&package).unwrap_err();
        }
    }

    #[test]
    fn feature_gated_binary_metadata_is_selected_without_resolving_features() {
        let mut package = package();
        package["targets"] = json!([{"name":"gated","kind":["bin"],"required-features":["cli"]}]);
        let selected = requests(&package).unwrap();
        assert_eq!(
            selected.first().unwrap().binary.as_ref().unwrap().name,
            "gated"
        );
    }

    #[test]
    fn reserved_package_metadata_uses_the_shared_schema() {
        for metadata in [
            json!({"release_targets":["x86_64-pc-windows-msvc"]}),
            json!({"release-targtes":["x86_64-pc-windows-msvc"]}),
            json!({"private-api":"true"}),
            json!({"release-targets":[false]}),
            json!({"groups":[]}),
        ] {
            let mut package = package();
            package["metadata"]["release-plan"] = metadata;
            requests(&package).unwrap_err();
        }
        let mut package = package();
        package["metadata"]["release-plan"] = json!({
            "private-api":true,"release-targets":["x86_64-pc-windows-msvc"]
        });
        requests(&package).unwrap();
    }

    #[test]
    fn registry_preflight_selects_only_permitted_destinations_without_binary_checks() {
        for (publish, included) in [
            (Value::Null, true),
            (json!([]), false),
            (json!(["private"]), false),
            (json!(["private", "crates-io"]), true),
        ] {
            let mut package = package();
            package["publish"] = publish;
            package["metadata"] = json!({});
            let workspace: PublicationWorkspace = serde_json::from_value(json!({
                "workspace_root":"workspace","workspace_members":["tool-id"],"packages":[package]
            }))
            .unwrap();
            let expected = if included {
                vec!["tool".to_owned()]
            } else {
                Vec::new()
            };
            assert_eq!(workspace.registry_targets(None).unwrap(), expected);
            assert_eq!(
                workspace
                    .registry_targets(Some(&["tool".to_owned()]))
                    .unwrap(),
                expected
            );
            assert!(
                workspace
                    .registry_targets(Some(&["another".to_owned()]))
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    fn library_requests_need_no_binary_metadata_and_private_members_do_not_publish() {
        let mut library = package();
        library["targets"] = json!([{"name":"library","kind":["lib"]}]);
        library["metadata"] = json!({});
        assert!(
            requests(&library)
                .unwrap()
                .first()
                .unwrap()
                .binary
                .is_none()
        );
        library["publish"] = json!([]);
        assert!(requests(&library).unwrap().is_empty());
        let mut foreign = package();
        foreign["id"] = json!("not-a-workspace-member");
        assert!(requests(&foreign).unwrap().is_empty());
    }

    #[test]
    fn captured_configuration_is_revalidated_before_selecting_requests() {
        let mut value = serde_json::to_value(configuration()).unwrap();
        value["schema-version"] = json!(2);
        let config: Configuration = serde_json::from_value(value).unwrap();
        let workspace: PublicationWorkspace = serde_json::from_value(json!({
            "workspace_root": "workspace", "workspace_members": [], "packages": []
        }))
        .unwrap();
        workspace.requests(&config).unwrap_err();
    }
}
