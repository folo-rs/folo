// Work-tree package discovery via `cargo metadata --no-deps`.
//
// The design forbids resolving a full graph or compiling. `--no-deps` is the
// only Cargo invocation used for classification.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use ohno::AppError;
use semver::Version;
use serde::Deserialize;
use serde_json::Value;

use crate::command::run_capture;
use crate::groups::Groups;
#[cfg(test)]
use crate::inherited::InheritedKeys;
use crate::manifest::{
    PackageManifest, WorkspaceInherit, parse_document, parse_package_manifest, repo_relative_path,
};
#[cfg(test)]
use crate::packaging::PackagingRules;
use crate::{
    GroupNameCollisionError, InvalidVersionError, MalformedDefaultBaseError,
    MalformedVersionGroupError, NonPublishableGroupMemberError, ParseMetadataError, ReadFileError,
    UnknownGroupMemberError,
};

/// Work-tree snapshot from `cargo metadata --no-deps`.
#[derive(Debug)]
pub(crate) struct WorkTree {
    pub(crate) workspace_root: PathBuf,
    pub(crate) packages: Vec<WorkPackage>,
    /// Manifest paths of every member, publishable or not.
    ///
    /// `apply` rewrites dependency requirements in all of them, because a
    /// non-publishable member can still pin a package the plan increments.
    pub(crate) member_manifests: Vec<PathBuf>,
    /// Declared package name of every member, keyed by its manifest directory.
    ///
    /// `apply` rewrites a `path` dependency only after resolving that path to a
    /// member directory declaring the same package, so a same-named package
    /// living outside the workspace is left alone.
    pub(crate) members_by_dir: BTreeMap<PathBuf, String>,
    pub(crate) groups: Groups,
    /// Base revision to classify against when the caller passes no `--base`.
    pub(crate) default_base: String,
}

/// One publishable workspace member in the work tree.
#[derive(Clone, Debug)]
pub(crate) struct WorkPackage {
    pub(crate) manifest: PackageManifest,
    pub(crate) manifest_path: PathBuf,
    pub(crate) dependencies: Vec<ReportedDep>,
    /// Files Cargo packs because a manifest key names them, keyed by the path
    /// each takes inside the `.crate`.
    ///
    /// Resolution needs the repository layout, which `cargo metadata` does not
    /// describe, so classification fills this in once the repository is known.
    pub(crate) resources: BTreeMap<String, String>,
}

/// Intra-workspace dependency as exposed in `report.json`.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct ReportedDep {
    pub(crate) name: String,
    pub(crate) req: String,
    pub(crate) exact_pin: bool,
}

/// Raw `cargo metadata` document before conversion to [`WorkTree`].
#[derive(Debug, Deserialize)]
struct MetadataJson {
    packages: Vec<MetadataPackage>,
    workspace_members: Vec<String>,
    workspace_root: String,
    #[serde(default)]
    metadata: Value,
}

/// One package object from the metadata document.
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

/// One declared dependency from the metadata document.
#[derive(Debug, Deserialize)]
struct MetadataDep {
    name: String,
    req: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    kind: Option<String>,
}

pub(crate) fn load_work_tree(manifest_path: &Path) -> Result<WorkTree, AppError> {
    // Cargo resolves a relative `--manifest-path` against the child's working
    // directory, so the child inherits this process's directory and the path is
    // passed through unchanged. Deriving the directory from the path instead
    // would resolve any leading directory component twice.
    let cwd = Path::new(".");
    // `--no-deps` is the classification Cargo invocation: no graph resolve and
    // no crates.io. `--offline` is omitted so a workspace without a lockfile
    // can still be classified; no registry packages are consulted.
    // The requested schema version is pinned because the `Metadata*`
    // projections in this module deserialize exactly that documented contract.
    let metadata = run_capture(
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
    let metadata: MetadataJson =
        serde_json::from_str(&metadata).map_err(ParseMetadataError::caused_by)?;

    let workspace_root = PathBuf::from(&metadata.workspace_root);
    let member_ids: HashSet<&str> = metadata
        .workspace_members
        .iter()
        .map(String::as_str)
        .collect();

    let workspace_names: HashSet<String> = metadata
        .packages
        .iter()
        .filter(|package| member_ids.contains(package.id.as_str()))
        .map(|package| package.name.clone())
        .collect();
    let members_by_dir: BTreeMap<PathBuf, String> = metadata
        .packages
        .iter()
        .filter(|package| member_ids.contains(package.id.as_str()))
        .filter_map(|package| {
            Path::new(&package.manifest_path)
                .parent()
                .map(|dir| (dir.to_path_buf(), package.name.clone()))
        })
        .collect();
    let root_manifest_path = workspace_root.join("Cargo.toml");
    let root_manifest = fs::read_to_string(&root_manifest_path)
        .map_err(|error| ReadFileError::caused_by(&root_manifest_path, error))?;
    let root_manifest = parse_document(&root_manifest_path, &root_manifest)?;
    let workspace = WorkspaceInherit::from_root(&root_manifest);
    let mut packages = Vec::new();

    for package in &metadata.packages {
        if !member_ids.contains(package.id.as_str()) {
            continue;
        }
        if matches!(&package.publish, Some(regs) if regs.is_empty()) {
            continue;
        }
        let path = PathBuf::from(&package.manifest_path);
        let manifest =
            fs::read_to_string(&path).map_err(|error| ReadFileError::caused_by(&path, error))?;
        let git_manifest_path = repo_relative_path(&workspace_root, &path);
        let Some(mut manifest) = parse_package_manifest(&manifest, &git_manifest_path, &workspace)?
        else {
            continue;
        };
        if !manifest.publish {
            continue;
        }
        manifest.version = package.version.parse::<Version>().map_err(|error| {
            InvalidVersionError::caused_by(&package.name, &package.version, error)
        })?;
        manifest.name.clone_from(&package.name);

        let dependencies = package
            .dependencies
            .iter()
            .filter(|dep| is_intra_workspace_released(dep, &members_by_dir))
            .map(|dep| ReportedDep {
                name: dep.name.clone(),
                req: dep.req.clone(),
                exact_pin: dep.req.starts_with('='),
            })
            .collect();

        packages.push(WorkPackage {
            manifest,
            manifest_path: path,
            dependencies,
            resources: BTreeMap::new(),
        });
    }

    packages.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));

    let mut member_manifests: Vec<PathBuf> = members_by_dir
        .keys()
        .map(|dir| dir.join("Cargo.toml"))
        .collect();
    member_manifests.sort();

    // Group configuration is validated once the publishable set is known,
    // because a group may only name packages that are actually released.
    let publishable_names: HashSet<&str> = packages
        .iter()
        .map(|package| package.manifest.name.as_str())
        .collect();
    let groups = groups_from_metadata(&metadata.metadata, &workspace_names, &publishable_names)?;
    let default_base = default_base_from_metadata(&metadata.metadata)?;

    Ok(WorkTree {
        workspace_root,
        packages,
        member_manifests,
        members_by_dir,
        groups,
        default_base,
    })
}

/// Base revision used when neither `--base` nor workspace metadata names one.
///
/// A repository that follows the common GitHub layout releases from the default
/// remote branch, so its tip is the revision a local run wants to compare
/// against. A repository that does not can say so in workspace metadata.
const FALLBACK_BASE: &str = "origin/main";

/// Reads the workspace-declared default base revision.
///
/// A repository whose mainline is not the fallback would otherwise have to pass
/// `--base` on every local invocation, and a stale default silently both adds
/// and hides differences.
fn default_base_from_metadata(metadata: &Value) -> Result<String, AppError> {
    let Some(base) = metadata
        .get("release-plan")
        .and_then(|plan| plan.get("base"))
    else {
        return Ok(FALLBACK_BASE.to_owned());
    };
    let Some(base) = base.as_str().filter(|base| !base.is_empty()) else {
        return Err(MalformedDefaultBaseError::new().into());
    };
    Ok(base.to_owned())
}

fn groups_from_metadata(
    metadata: &Value,
    workspace_names: &HashSet<String>,
    publishable_names: &HashSet<&str>,
) -> Result<Groups, AppError> {
    let Some(groups) = metadata
        .get("release-plan")
        .and_then(|plan| plan.get("groups"))
        .and_then(Value::as_object)
    else {
        return Ok(Groups::default());
    };
    let mut map = BTreeMap::new();
    for (name, members) in groups {
        let Some(array) = members.as_array() else {
            return Err(MalformedVersionGroupError::new(name).into());
        };
        let mut parsed = Vec::new();
        for item in array {
            let Some(package) = item.as_str() else {
                return Err(MalformedVersionGroupError::new(name).into());
            };
            if !workspace_names.contains(package) {
                return Err(UnknownGroupMemberError::new(name, package).into());
            }
            if !publishable_names.contains(package) {
                return Err(NonPublishableGroupMemberError::new(name, package).into());
            }
            parsed.push(package.to_owned());
        }
        // A plan entry names either a package or a group, and a group wins the
        // lookup. Naming a group after a package it does not contain would
        // therefore make an entry increment a different set of packages than
        // the one its author named, silently.
        if workspace_names.contains(name) && !parsed.iter().any(|member| member == name) {
            return Err(GroupNameCollisionError::new(name).into());
        }
        map.insert(name.clone(), parsed);
    }
    Groups::from_members(map)
}

/// Reports whether a dependency edge is published and points at a workspace member.
///
/// Normal and build dependencies are recorded in the published manifest, so a
/// version decision on the target cascades to this package. A dev dependency
/// survives packaging only when it declares a version requirement; Cargo strips
/// the path-only ones, whose requirement `cargo metadata` reports as the `*`
/// default, and those cannot cascade.
fn is_intra_workspace_released(
    dep: &MetadataDep,
    members_by_dir: &BTreeMap<PathBuf, String>,
) -> bool {
    let kind = dep.kind.as_deref().unwrap_or("normal");
    if kind == "dev" && dep.req == "*" {
        return false;
    }
    let Some(path) = &dep.path else {
        return false;
    };
    members_by_dir.contains_key(Path::new(path))
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
    use serde_json::json;

    use super::*;

    /// Group configuration is checked against the publishable set, which for
    /// most cases is simply every workspace member.
    fn all_publishable(names: &HashSet<String>) -> HashSet<&str> {
        names.iter().map(String::as_str).collect()
    }

    #[test]
    fn groups_from_metadata_reads_release_plan_table() {
        let json = json!({
            "release-plan": {
                "groups": {
                    "nm": ["nm", "nm_impl"]
                }
            }
        });
        let names = HashSet::from(["nm".to_string(), "nm_impl".to_string()]);
        let groups = groups_from_metadata(&json, &names, &all_publishable(&names)).unwrap();
        assert_eq!(groups.group_of("nm_impl"), Some("nm"));
    }

    #[test]
    fn default_base_falls_back_when_the_workspace_declares_none() {
        assert_eq!(
            default_base_from_metadata(&json!({})).unwrap(),
            FALLBACK_BASE
        );
        assert_eq!(
            default_base_from_metadata(&json!({ "release-plan": {} })).unwrap(),
            FALLBACK_BASE
        );
    }

    #[test]
    fn default_base_reads_the_workspace_declaration() {
        let json = json!({ "release-plan": { "base": "origin/trunk" } });
        assert_eq!(default_base_from_metadata(&json).unwrap(), "origin/trunk");
    }

    /// A base that is not a usable revision name would otherwise surface as a
    /// confusing `git rev-parse` failure much later.
    #[test]
    fn default_base_rejects_a_non_revision_declaration() {
        for json in [
            json!({ "release-plan": { "base": 1 } }),
            json!({ "release-plan": { "base": "" } }),
        ] {
            let error = default_base_from_metadata(&json).unwrap_err();
            assert!(error.find_source::<MalformedDefaultBaseError>().is_some());
        }
    }

    /// A version group keeps released versions in lockstep, so a member that is
    /// never published has no version to keep in step and would otherwise be
    /// dropped from every decision without a word.
    #[test]
    fn groups_from_metadata_rejects_a_non_publishable_member() {
        let json = json!({
            "release-plan": { "groups": { "nm": ["nm", "nm_impl"] } }
        });
        let names = HashSet::from(["nm".to_string(), "nm_impl".to_string()]);
        let publishable = HashSet::from(["nm"]);

        let error = groups_from_metadata(&json, &names, &publishable).unwrap_err();

        let reported = error
            .find_source::<NonPublishableGroupMemberError>()
            .expect("a non-publishable member is refused")
            .to_string();
        assert!(reported.contains("nm_impl"), "{reported}");
    }

    #[test]
    fn groups_from_metadata_rejects_malformed_and_unknown_members() {
        let names = HashSet::from(["nm".to_string()]);
        let malformed = json!({
            "release-plan": { "groups": { "nm": "nm" } }
        });
        let error = groups_from_metadata(&malformed, &names, &all_publishable(&names)).unwrap_err();
        let source = error
            .find_source::<MalformedVersionGroupError>()
            .expect("malformed group");
        assert_eq!(source.group(), "nm");
        let non_string = json!({
            "release-plan": { "groups": { "nm": [1] } }
        });
        let error =
            groups_from_metadata(&non_string, &names, &all_publishable(&names)).unwrap_err();
        let source = error
            .find_source::<MalformedVersionGroupError>()
            .expect("malformed group member");
        assert_eq!(source.group(), "nm");
        let unknown = json!({
            "release-plan": { "groups": { "nm": ["ghost"] } }
        });
        let error = groups_from_metadata(&unknown, &names, &all_publishable(&names)).unwrap_err();
        let source = error
            .find_source::<UnknownGroupMemberError>()
            .expect("unknown member");
        assert_eq!(source.group(), "nm");
        assert_eq!(source.package(), "ghost");
    }

    #[test]
    fn a_group_named_after_a_package_must_contain_that_package() {
        let names = HashSet::from(["nm".to_string(), "nm_impl".to_string()]);
        let collision = json!({
            "release-plan": { "groups": { "nm": ["nm_impl"] } }
        });
        let error = groups_from_metadata(&collision, &names, &all_publishable(&names)).unwrap_err();
        let source = error
            .find_source::<GroupNameCollisionError>()
            .expect("group name collision");
        assert_eq!(source.group(), "nm");

        // A group name that is not a package name is unambiguous, and so is one
        // that names a package it does contain.
        let free_name = json!({
            "release-plan": { "groups": { "nm-family": ["nm_impl"] } }
        });
        groups_from_metadata(&free_name, &names, &all_publishable(&names)).unwrap();
        let contains_itself = json!({
            "release-plan": { "groups": { "nm": ["nm", "nm_impl"] } }
        });
        groups_from_metadata(&contains_itself, &names, &all_publishable(&names)).unwrap();
    }

    #[test]
    fn released_intra_workspace_deps_require_a_member_directory() {
        let dirs = BTreeMap::from([(PathBuf::from("/ws/packages/bar"), "bar".to_string())]);
        let path_dep = MetadataDep {
            name: "bar".to_string(),
            req: "0.1.0".to_string(),
            path: Some("/ws/packages/bar".to_string()),
            kind: None,
        };
        assert!(is_intra_workspace_released(&path_dep, &dirs));
        let named = MetadataDep {
            name: "bar".to_string(),
            req: "0.1.0".to_string(),
            path: None,
            kind: Some("normal".to_string()),
        };
        assert!(!is_intra_workspace_released(&named, &dirs));
        let colliding = MetadataDep {
            name: "bar".to_string(),
            req: "0.1.0".to_string(),
            path: Some("/other/bar".to_string()),
            kind: None,
        };
        assert!(!is_intra_workspace_released(&colliding, &dirs));
        let build = MetadataDep {
            name: "bar".to_string(),
            req: "0.1.0".to_string(),
            path: Some("/ws/packages/bar".to_string()),
            kind: Some("build".to_string()),
        };
        assert!(is_intra_workspace_released(&build, &dirs));
        let dev = MetadataDep {
            name: "bar".to_string(),
            req: "0.1.0".to_string(),
            path: Some("/ws/packages/bar".to_string()),
            kind: Some("dev".to_string()),
        };
        assert!(is_intra_workspace_released(&dev, &dirs));
        let path_only_dev = MetadataDep {
            name: "bar".to_string(),
            req: "*".to_string(),
            path: Some("/ws/packages/bar".to_string()),
            kind: Some("dev".to_string()),
        };
        assert!(!is_intra_workspace_released(&path_only_dev, &dirs));
        // A normal dependency without a version requirement still survives
        // packaging, because Cargo strips only path-only dev dependencies.
        let path_only_normal = MetadataDep {
            name: "bar".to_string(),
            req: "*".to_string(),
            path: Some("/ws/packages/bar".to_string()),
            kind: None,
        };
        assert!(is_intra_workspace_released(&path_only_normal, &dirs));
        let foreign = MetadataDep {
            name: "serde".to_string(),
            req: "1.0.0".to_string(),
            path: None,
            kind: None,
        };
        assert!(!is_intra_workspace_released(&foreign, &dirs));
    }

    #[test]
    fn dependents_of_lists_packages_that_depend_on_the_name() {
        fn package(name: &str, dependencies: Vec<ReportedDep>) -> WorkPackage {
            WorkPackage {
                manifest: PackageManifest {
                    name: name.to_string(),
                    version: "0.1.0".parse().unwrap(),
                    directory: format!("packages/{name}"),
                    packaging: PackagingRules::default(),
                    inherited: InheritedKeys::default(),
                    publish: true,
                    path_dependencies: Vec::new(),
                    inherited_path_dependencies: Vec::new(),
                    resource_paths: Vec::new(),
                    inherited_resource_paths: Vec::new(),
                    auto_readme: false,
                },
                manifest_path: PathBuf::from(format!("packages/{name}/Cargo.toml")),
                dependencies,
                resources: BTreeMap::new(),
            }
        }

        let bar = package(
            "bar",
            vec![ReportedDep {
                name: "foo".to_string(),
                req: "0.1.0".to_string(),
                exact_pin: false,
            }],
        );
        let foo = package("foo", Vec::new());
        assert_eq!(dependents_of(&[foo, bar], "foo"), vec!["bar".to_string()]);
    }
}
