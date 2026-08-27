// Work-tree package discovery via `cargo metadata --no-deps`.
//
// The design forbids resolving a full graph or compiling. `--no-deps` is the
// only Cargo invocation used for classification.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use ohno::AppError;
use semver::Version;
use serde::Deserialize;

use crate::ParseMetadataError;
use crate::command::run_capture;
use crate::groups::Groups;
#[cfg(test)]
use crate::inherited::InheritedKeys;
use crate::manifest::{PackageManifest, parse_package_manifest, repo_relative_dir};
#[cfg(test)]
use crate::packaging::PackagingRules;

#[derive(Debug, Deserialize)]
struct MetadataJson {
    packages: Vec<MetadataPackage>,
    workspace_members: Vec<String>,
    workspace_root: String,
    #[serde(default)]
    metadata: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct MetadataPackage {
    name: String,
    version: String,
    id: String,
    manifest_path: String,
    #[serde(default)]
    publish: Option<Vec<String>>,
    #[serde(default)]
    dependencies: Vec<MetadataDep>,
}

#[derive(Debug, Deserialize)]
struct MetadataDep {
    name: String,
    req: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    kind: Option<String>,
}

/// One publishable workspace member in the work tree.
#[derive(Clone, Debug)]
pub(crate) struct WorkPackage {
    pub manifest: PackageManifest,
    pub manifest_path: PathBuf,
    pub dependencies: Vec<ReportedDep>,
}

/// Intra-workspace dependency as exposed in `report.json`.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct ReportedDep {
    pub name: String,
    pub req: String,
    pub exact_pin: bool,
}

/// Work-tree snapshot from `cargo metadata --no-deps`.
#[derive(Debug)]
pub(crate) struct WorkTree {
    pub workspace_root: PathBuf,
    pub packages: Vec<WorkPackage>,
    pub groups: Groups,
}

pub(crate) fn load_work_tree(manifest_path: &Path) -> Result<WorkTree, AppError> {
    let cwd = manifest_path.parent().unwrap_or(manifest_path);
    // `--no-deps` is the classification Cargo invocation: no graph resolve and
    // no crates.io. `--offline` is omitted so a workspace without a lockfile
    // can still be classified; no registry packages are consulted.
    let stdout = run_capture(
        "cargo",
        &[
            "metadata",
            "--no-deps",
            "--format-version",
            "1",
            "--manifest-path",
            &manifest_path.to_string_lossy(),
        ],
        cwd,
    )?;
    let json: MetadataJson =
        serde_json::from_str(&stdout).map_err(ParseMetadataError::caused_by)?;

    let workspace_root = PathBuf::from(&json.workspace_root);
    let member_ids: HashSet<&str> = json.workspace_members.iter().map(String::as_str).collect();

    let groups = groups_from_metadata(&json.metadata)?;
    let mut packages = Vec::new();

    for package in &json.packages {
        if !member_ids.contains(package.id.as_str()) {
            continue;
        }
        if matches!(&package.publish, Some(regs) if regs.is_empty()) {
            continue;
        }
        let path = PathBuf::from(&package.manifest_path);
        let content = std::fs::read_to_string(&path)
            .map_err(|error| crate::ReadFileError::caused_by(&path, error))?;
        let Some(mut parsed) = parse_package_manifest(
            &content,
            &path.to_string_lossy(),
            Some(package.version.as_str()),
        )?
        else {
            continue;
        };
        if !parsed.publish {
            continue;
        }
        parsed.directory = repo_relative_dir(&workspace_root, &path);
        parsed.version = package.version.parse::<Version>().map_err(|error| {
            crate::InvalidVersionError::caused_by(&package.name, &package.version, error)
        })?;
        parsed.name.clone_from(&package.name);

        let workspace_names: HashSet<&str> =
            json.packages.iter().map(|p| p.name.as_str()).collect();
        let dependencies = package
            .dependencies
            .iter()
            .filter(|dep| is_intra_workspace_normal(dep, &workspace_names))
            .map(|dep| ReportedDep {
                name: dep.name.clone(),
                req: dep.req.clone(),
                exact_pin: dep.req.starts_with('='),
            })
            .collect();

        packages.push(WorkPackage {
            manifest: parsed,
            manifest_path: path,
            dependencies,
        });
    }

    packages.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));

    Ok(WorkTree {
        workspace_root,
        packages,
        groups,
    })
}

fn groups_from_metadata(metadata: &serde_json::Value) -> Result<Groups, AppError> {
    let Some(groups) = metadata
        .get("release-plan")
        .and_then(|plan| plan.get("groups"))
        .and_then(serde_json::Value::as_object)
    else {
        return Ok(Groups::default());
    };
    let mut map = BTreeMap::new();
    for (name, members) in groups {
        let Some(array) = members.as_array() else {
            continue;
        };
        let members = array
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        map.insert(name.clone(), members);
    }
    Groups::from_members(map)
}

fn is_intra_workspace_normal(dep: &MetadataDep, workspace_names: &HashSet<&str>) -> bool {
    let kind = dep.kind.as_deref().unwrap_or("normal");
    kind == "normal" && (dep.path.is_some() || workspace_names.contains(dep.name.as_str()))
}

pub(crate) fn dependents_of(packages: &[WorkPackage], name: &str) -> Vec<String> {
    packages
        .iter()
        .filter(|package| package.dependencies.iter().any(|dep| dep.name == name))
        .map(|package| package.manifest.name.clone())
        .collect()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn groups_from_metadata_reads_release_plan_table() {
        let json = serde_json::json!({
            "release-plan": {
                "groups": {
                    "nm": ["nm", "nm_impl"]
                }
            }
        });
        let groups = groups_from_metadata(&json).unwrap();
        assert_eq!(groups.group_of("nm_impl"), Some("nm"));
    }

    #[test]
    fn intra_workspace_normal_deps_require_path_or_workspace_name() {
        let names = HashSet::from(["bar"]);
        let path_dep = MetadataDep {
            name: "bar".to_string(),
            req: "0.1.0".to_string(),
            path: Some("../bar".to_string()),
            kind: None,
        };
        assert!(is_intra_workspace_normal(&path_dep, &names));
        let named = MetadataDep {
            name: "bar".to_string(),
            req: "0.1.0".to_string(),
            path: None,
            kind: Some("normal".to_string()),
        };
        assert!(is_intra_workspace_normal(&named, &names));
        let dev = MetadataDep {
            name: "bar".to_string(),
            req: "0.1.0".to_string(),
            path: Some("../bar".to_string()),
            kind: Some("dev".to_string()),
        };
        assert!(!is_intra_workspace_normal(&dev, &names));
        let foreign = MetadataDep {
            name: "serde".to_string(),
            req: "1.0.0".to_string(),
            path: None,
            kind: None,
        };
        assert!(!is_intra_workspace_normal(&foreign, &names));
    }

    #[test]
    fn dependents_of_lists_packages_that_depend_on_the_name() {
        let bar = WorkPackage {
            manifest: PackageManifest {
                name: "bar".to_string(),
                version: "0.1.0".parse().unwrap(),
                directory: "packages/bar".to_string(),
                packaging: PackagingRules::default(),
                inherited: InheritedKeys::default(),
                publish: true,
            },
            manifest_path: PathBuf::from("packages/bar/Cargo.toml"),
            dependencies: vec![ReportedDep {
                name: "foo".to_string(),
                req: "0.1.0".to_string(),
                exact_pin: false,
            }],
        };
        let foo = WorkPackage {
            manifest: PackageManifest {
                name: "foo".to_string(),
                version: "0.1.0".parse().unwrap(),
                directory: "packages/foo".to_string(),
                packaging: PackagingRules::default(),
                inherited: InheritedKeys::default(),
                publish: true,
            },
            manifest_path: PathBuf::from("packages/foo/Cargo.toml"),
            dependencies: vec![],
        };
        assert_eq!(dependents_of(&[foo, bar], "foo"), vec!["bar".to_string()]);
    }
}
