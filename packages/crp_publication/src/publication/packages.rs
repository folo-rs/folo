//! Package-driven publication validation over Cargo's unresolved metadata.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crp_diag::Verbose;
use crp_workspace::metadata::capture_metadata;
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
        serde_json::from_str(&source).map_err(|error| ParseMetadataError::caused_by(error).into())
    }

    /// Returns all publication requests, including versions already available remotely.
    pub fn requests(&self, config: &Configuration) -> Result<Vec<PackageRequest>, AppError> {
        config.validate(Path::new(".cargo/release_plan.toml"))?;
        let mut requests = Vec::new();
        for package in &self.packages {
            if !self.workspace_members.contains(&package.id)
                || package.publish.as_ref().is_some_and(Vec::is_empty)
            {
                continue;
            }
            if package
                .publish
                .as_ref()
                .is_some_and(|registries| !registries.iter().any(|name| name == "crates-io"))
            {
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
                    validate_binary(package, target, config)?;
                    if package
                        .metadata
                        .get("release-plan")
                        .is_some_and(|value| !value.is_object())
                    {
                        return Err(PackageConfigurationError::new(
                            &package.name,
                            "metadata.release-plan must be a table".to_owned(),
                        )
                        .into());
                    }
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
}

/// Validated package selection, before repository-relative publication identity is captured.
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

/// Publication-specific projection of Cargo metadata, including feature-gated binary inputs.
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
    features: BTreeMap<String, Vec<String>>,
}

/// One Cargo target and the features required to build it.
#[derive(Debug, Deserialize)]
struct PublicationTarget {
    name: String,
    kind: Vec<String>,
    #[serde(default, rename = "required-features")]
    required_features: Vec<String>,
}

fn validate_binary(
    package: &PublicationPackage,
    target: &PublicationTarget,
    config: &Configuration,
) -> Result<(), AppError> {
    let mut selected = BTreeSet::new();
    let mut pending = vec!["default".to_owned()];
    while let Some(feature) = pending.pop() {
        if selected.insert(feature.clone()) {
            if let Some(features) = package.features.get(&feature) {
                pending.extend(features.iter().cloned());
            }
            // Strong dependency-feature forwarding enables an optional dependency;
            // the weak `dependency?/feature` form does not.
            if let Some((dependency, _)) = feature.split_once('/')
                && !dependency.ends_with('?')
            {
                pending.push(dependency.to_owned());
            }
        }
    }
    if target
        .required_features
        .iter()
        .any(|feature| !selected.contains(feature))
    {
        return Err(PackageConfigurationError::new(
            &package.name,
            "the binary requires features not enabled by default".to_owned(),
        )
        .into());
    }
    let repository = format!("https://github.com/{}", config.repository());
    if package.repository.as_deref() != Some(repository.as_str()) {
        return Err(PackageConfigurationError::new(
            &package.name,
            format!("binary repository must be {repository} for binstall asset URLs"),
        )
        .into());
    }
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
            "features": {},
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
    fn rejects_unreachable_or_ambiguous_binary_and_archive_promises() {
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
        let mut gated = package();
        gated["targets"] = json!([{"name":"gated","kind":["bin"],"required-features":["cli"]}]);
        cases.push(gated);
        for package in cases {
            requests(&package).unwrap_err();
        }
    }

    #[test]
    fn default_feature_closure_enables_required_binary_features() {
        let mut package = package();
        package["features"] = json!({"default":["full"], "full":["cli"], "cli":["full"]});
        package["targets"] = json!([{"name":"gated","kind":["bin"],"required-features":["cli"]}]);
        requests(&package).unwrap();
        package["features"] = json!({"default":["optional/enhanced"], "optional":["dep:optional"]});
        package["targets"] =
            json!([{"name":"gated","kind":["bin"],"required-features":["optional"]}]);
        requests(&package).unwrap();
        package["features"] =
            json!({"default":["optional?/enhanced"], "optional":["dep:optional"]});
        requests(&package).unwrap_err();
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
